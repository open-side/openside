use super::*;

type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

const ITEM: &str = r#"{"id":"bank","origins":["https://bank.example"],"policy":null,
"effective_policy":"while_unlocked","has_totp":true,"passkey_rp_ids":["bank.example"]}"#;

/// Fake daemon on a Unix socket answering `/v1/vault/*`.
async fn daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("vault.sock");
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
                let item: Value = serde_json::from_str(ITEM).unwrap();
                match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/vault/items") => respond(
                        200,
                        json!({"available":true,"default_policy":"while_unlocked","items":[item]}),
                    ),
                    ("PATCH", "/v1/vault/items/bank") => {
                        let mut item = item;
                        item["policy"] = json!("never");
                        item["effective_policy"] = json!("never");
                        respond(200, item)
                    }
                    ("PATCH", _) => respond(404, json!({"code":"unknown_item","message":"unknown_item"})),
                    ("GET", "/v1/vault") => respond(
                        200,
                        json!({"state":"uninitialized","backend":"passphrase","reason":null,"default_policy":"while_unlocked"}),
                    ),
                    ("POST", "/v1/vault/setup" | "/v1/vault/unlock") => respond(
                        200,
                        json!({"state":"unlocked","backend":"passphrase","reason":null,"default_policy":"while_unlocked"}),
                    ),
                    ("POST", "/v1/vault/items") => respond(201, item),
                    ("DELETE", "/v1/vault/items/bank") => Response::builder()
                        .status(204)
                        .body(Body::empty())
                        .unwrap(),
                    _ => respond(
                        200,
                        json!({"records":[{"at":"2026-01-01T00:00:00Z","who":"opensided","what":"fill",
                        "item":"bank","origin":"https://bank.example","result":"denied","session_id":"s-1",
                        "tool_call_id":"call-1","detail":"policy_never"}]}),
                    ),
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

async fn cli_input(socket: &std::path::Path, args: &[&str], input: &str) -> (u8, String, String) {
    let mut argv = vec![
        OsString::from("openside"),
        OsString::from("--socket"),
        socket.as_os_str().to_owned(),
    ];
    argv.extend(args.iter().map(OsString::from));
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let code = run_with_input(argv, &mut input.as_bytes(), &mut stdout, &mut stderr).await;
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

/// `init`, `add` and `rm` read values one per line from piped stdin, send them only in
/// the request body and never print them.
#[tokio::test]
async fn vault_init_add_and_rm_take_values_from_stdin_and_never_print_them() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    let last = || seen.lock().last().unwrap().clone();

    let (code, out, err) = cli(&socket, &["vault", "status"]).await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "uninitialized\tpassphrase\n");

    let (code, _, err) = cli_input(
        &socket,
        &["vault", "init"],
        "first-passphrase\nother-passphrase\n",
    )
    .await;
    assert_eq!(code, 1);
    assert!(err.contains("do not match"), "{err}");
    assert_eq!(last().1, "/v1/vault", "a mismatch sends nothing");
    let (code, out, err) = cli_input(
        &socket,
        &["vault", "init"],
        "first-passphrase\nfirst-passphrase\n",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "unlocked\tpassphrase\n");
    assert_eq!(
        last(),
        (
            "POST".into(),
            "/v1/vault/setup".into(),
            json!({"passphrase":"first-passphrase"})
        )
    );
    assert!(!err.contains("first-passphrase"));

    let (code, out, err) = cli_input(
        &socket,
        &[
            "vault",
            "add",
            "bank",
            "--origin",
            "https://bank.example",
            "--login",
            "--totp",
            "--policy",
            "never",
        ],
        "person@example.com\nhunter2-canary\nJBSWY3DPEHPK3PXP\n",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        last(),
        (
            "POST".into(),
            "/v1/vault/items".into(),
            json!({"id":"bank","origins":["https://bank.example"],"username":"person@example.com",
                "password":"hunter2-canary","totp":"JBSWY3DPEHPK3PXP","policy":"never"})
        )
    );
    for value in ["person@example.com", "hunter2-canary", "JBSWY3DPEHPK3PXP"] {
        assert!(
            !out.contains(value) && !err.contains(value),
            "printed a value"
        );
    }
    assert!(out.starts_with("bank\t"), "{out}");

    let (code, _, _) = cli_input(
        &socket,
        &[
            "vault",
            "add",
            "gh",
            "--origin",
            "https://github.com",
            "--totp",
        ],
        "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n",
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(
        last().2,
        json!({"id":"gh","origins":["https://github.com"],"username":null,"password":null,
            "totp":"otpauth://totp/x?secret=JBSWY3DPEHPK3PXP","policy":null})
    );
    let (code, ..) = cli(&socket, &["vault", "add", "gh"]).await;
    assert_eq!(code, 2, "--origin is required");

    let (code, out, _) = cli(&socket, &["vault", "rm", "bank"]).await;
    assert_eq!((code, out.as_str()), (0, "Deleted bank\n"));
    assert_eq!(last().0, "DELETE");
    server.abort();
}

#[tokio::test]
async fn vault_items_policy_and_audit_commands() {
    let environment = Environment::new().await;
    let (socket, seen, server) = daemon(&environment).await;
    let last = || seen.lock().last().unwrap().clone();

    let (code, out, err) = cli(&socket, &["vault", "items"]).await;
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(
        out,
        "bank\twhile_unlocked (default)\thttps://bank.example\ttotp=yes\tpasskeys=bank.example\n"
    );
    assert_eq!(last().1, "/v1/vault/items");

    let (code, out, _) = cli(&socket, &["vault", "policy", "bank", "never"]).await;
    assert_eq!(code, 0);
    assert_eq!(
        last(),
        (
            "PATCH".into(),
            "/v1/vault/items/bank".into(),
            json!({"policy":"never"})
        )
    );
    assert!(out.starts_with("bank\tnever\t"), "{out}");
    let (code, ..) = cli(&socket, &["vault", "policy", "a/b", "default"]).await;
    assert_eq!(code, 1);
    assert_eq!(
        last(),
        (
            "PATCH".into(),
            "/v1/vault/items/a%2Fb".into(),
            json!({"policy":"default"})
        )
    );
    let (code, ..) = cli(&socket, &["vault", "policy", "bank", "sometimes"]).await;
    assert_eq!(code, 2, "clap rejects unknown policies before any request");

    let (code, out, _) = cli(
        &socket,
        &[
            "vault",
            "audit",
            "--since",
            "2026-01-01T00:00:00Z",
            "--item",
            "bank",
            "--session",
            "s-1",
            "--limit",
            "5",
        ],
    )
    .await;
    assert_eq!(code, 0);
    assert_eq!(
        last().1,
        "/v1/vault/audit?since=2026-01-01T00%3A00%3A00Z&item=bank&session=s-1&limit=5"
    );
    assert_eq!(
        out,
        "2026-01-01T00:00:00+00:00\ts-1\tcall-1\tfill\tbank\tdenied (policy_never)\thttps://bank.example\topensided\n"
    );
    let (code, out, _) = cli(&socket, &["--json", "vault", "audit"]).await;
    assert_eq!(code, 0);
    assert_eq!(last().1, "/v1/vault/audit");
    let parsed: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["records"][0]["detail"], "policy_never");
    server.abort();
}

const SETTINGS: &str = r#"{"auto_lock":"one_week","biometric_unlock":false,
"biometrics":{"available":false,"mechanism":null,"reason":"passphrase_vault"},
"autofill":{"enabled":true,"auto_submit":true,"allow_insecure":false,"excluded_domains":["old.example"]},
"default_policy":"while_unlocked","disable_in_incognito":true,
"recovery":{"configured":false,"current":false,"created_at":null}}"#;

/// Fake daemon with an unlocked passphrase vault, for the password-settings commands.
async fn unlocked_daemon(environment: &Environment) -> (PathBuf, Seen, JoinHandle<()>) {
    let socket = environment.directory.path().join("passwords.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(
            |State(seen): State<Seen>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock().push((method.clone(), path.clone(), body));
                let unlocked = json!({"state":"unlocked","backend":"passphrase","reason":null,"default_policy":"while_unlocked"});
                match (method.as_str(), path.as_str()) {
                    ("GET", "/v1/vault") => respond(200, unlocked),
                    ("GET" | "PATCH", "/v1/vault/settings") => {
                        respond(200, serde_json::from_str(SETTINGS).unwrap())
                    }
                    ("POST", "/v1/vault/export") => respond(
                        200,
                        json!({"format":"csv","filename":"openside-passwords.csv",
                        "content":"name,url,username,password,otpauth\nbank,https://bank.example/,person,cli-export-canary,\n",
                        "exported":1,"skipped":2}),
                    ),
                    ("POST", "/v1/vault/clear") => respond(200, json!({"removed":3})),
                    ("POST", "/v1/vault/recovery-key") => respond(
                        200,
                        json!({"recovery_key":"AAAA-BBBB","created_at":"2026-01-01T00:00:00Z"}),
                    ),
                    ("POST", "/v1/vault/recover") => respond(200, unlocked),
                    ("POST", "/v1/import/passwords") => respond(
                        200,
                        json!({"read":2,"imported":1,"skipped":1,"rejected":[],"note":null}),
                    ),
                    _ => respond(404, json!({"code":"not_found","message":"not_found"})),
                }
            },
        )
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (socket, seen, server)
}

/// Settings, export, import, clear, recovery-key and recover: confirmation flags, the
/// passphrase prompt for fresh authentication, a private export file, and no value on stdout
/// except the recovery key that is meant to be written down.
#[tokio::test]
async fn password_settings_export_import_clear_and_recovery_commands() {
    let environment = Environment::new().await;
    let (socket, seen, server) = unlocked_daemon(&environment).await;
    let last = || seen.lock().last().unwrap().clone();

    let (code, out, err) = cli(&socket, &["vault", "settings"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("auto-lock\t1 week") && out.contains("old.example"),
        "{out}"
    );
    let (code, _, err) = cli(
        &socket,
        &[
            "vault",
            "settings",
            "--auto-lock",
            "5m",
            "--auto-submit",
            "false",
            "--exclude-domain",
            "bank.example",
            "--include-domain",
            "OLD.example",
        ],
    )
    .await;
    assert_eq!(code, 0, "{err}");
    let (method, path, body) = last();
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("PATCH", "/v1/vault/settings")
    );
    assert_eq!(body["auto_lock"], "five_minutes");
    assert_eq!(body["autofill"]["auto_submit"], false);
    assert_eq!(
        body["autofill"]["excluded_domains"],
        json!(["bank.example"])
    );
    assert!(body["autofill"]["enabled"].is_null() && body["biometric_unlock"].is_null());

    let file = environment.directory.path().join("passwords.csv");
    let target = file.to_str().unwrap();
    let (code, _, err) = cli(&socket, &["vault", "export", "--output", target]).await;
    assert_eq!(code, 1);
    assert!(err.contains("--yes"), "{err}");
    assert!(!file.exists());
    let (code, out, err) = cli_input(
        &socket,
        &["vault", "export", "--output", target, "--yes"],
        "vault-pass\n",
    )
    .await;
    assert_eq!(code, 0, "{err}");
    assert!(!out.contains("cli-export-canary") && !err.contains("cli-export-canary"));
    assert!(
        out.contains("Exported 1") && out.contains("2 passkeys"),
        "{out}"
    );
    let (_, path, body) = last();
    assert_eq!(path, "/v1/vault/export");
    assert_eq!(
        (&body["confirm"], &body["passphrase"]),
        (&json!(true), &json!("vault-pass"))
    );
    assert!(std::fs::read_to_string(&file)
        .unwrap()
        .contains("cli-export-canary"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (code, _, err) = cli_input(
        &socket,
        &["vault", "export", "--output", target, "--yes"],
        "vault-pass\n",
    )
    .await;
    assert_eq!(code, 1);
    assert!(err.contains("already exists"), "{err}");

    let (code, out, err) = cli(&socket, &["vault", "import", target]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "Imported 1 of 2 sign-ins (1 skipped)\n");
    assert_eq!(last().1, "/v1/import/passwords");
    assert!(last().2["csv"]
        .as_str()
        .unwrap()
        .contains("cli-export-canary"));
    // UTF-16 exports with a byte-order mark are decoded before sending; other bytes refuse.
    let wide = environment.directory.path().join("wide.csv");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "url,username,password\nhttps://wide.example,w,wide-canary\n".encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    std::fs::write(&wide, bytes).unwrap();
    let (code, _, err) = cli(&socket, &["vault", "import", wide.to_str().unwrap()]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        last().2["csv"],
        "url,username,password\nhttps://wide.example,w,wide-canary\n"
    );
    std::fs::write(&wide, [0x80, 0x81, 0x82]).unwrap();
    let (code, _, err) = cli(&socket, &["vault", "import", wide.to_str().unwrap()]).await;
    assert_eq!(code, 1);
    assert!(err.contains("UTF-16"), "{err}");

    let (code, _, err) = cli(&socket, &["vault", "clear"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("--yes"), "{err}");
    let (code, out, err) = cli_input(&socket, &["vault", "clear", "--yes"], "vault-pass\n").await;
    assert_eq!((code, out.as_str()), (0, "Deleted 3 items\n"), "{err}");
    assert_eq!(
        (&last().2["confirm"], &last().2["passphrase"]),
        (&json!(true), &json!("vault-pass"))
    );

    let (code, out, err) =
        cli_input(&socket, &["vault", "recovery-key", "--yes"], "vault-pass\n").await;
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("AAAA-BBBB\n"), "{out}");

    let (code, _, err) = cli_input(
        &socket,
        &["vault", "recover"],
        "AAAA-BBBB\nnew-passphrase\nnew-passphrase\n",
    )
    .await;
    // The fake reports the vault unlocked, so recovery is refused before anything is sent.
    assert_eq!(code, 1);
    assert!(err.contains("already unlocked"), "{err}");
    assert_ne!(last().1, "/v1/vault/recover");
    server.abort();
}

/// `vault init` on a keyring vault asks the daemon for the biometric check, which it runs
/// only when biometric unlock is on; a passphrase vault sends its passphrase instead.
#[tokio::test]
async fn keyring_init_asks_for_the_biometric_check() {
    let environment = Environment::new().await;
    let socket = environment.directory.path().join("keyring.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen: Seen = Arc::default();
    let router = Router::new()
        .fallback(
            |State(seen): State<Seen>, request: Request<Body>| async move {
                let method = request.method().to_string();
                let path = request.uri().path().to_owned();
                let bytes = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                seen.lock().push((method.clone(), path.clone(), body));
                let state = if method == "GET" { "uninitialized" } else { "unlocked" };
                respond(
                    200,
                    json!({"state":state,"backend":"secret_service","reason":null,"default_policy":"while_unlocked"}),
                )
            },
        )
        .with_state(seen.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (code, out, err) = cli(&socket, &["vault", "init"]).await;
    assert_eq!(code, 0, "{err}");
    assert_eq!(out, "unlocked\tsecret_service\n");
    assert_eq!(
        seen.lock().last().unwrap().clone(),
        (
            "POST".into(),
            "/v1/vault/setup".into(),
            json!({"passphrase":null,"biometric":true})
        )
    );
    server.abort();
}
