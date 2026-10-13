//! Vault through the daemon: per-item policy refusal, audit with session/tool-call ids,
//! value-free item/audit APIs, redirect/phishing origin refusal (C-3/C-4/C-5) and the
//! Full-access no-value-path sweep over every tool and GET route (D-6).

use super::*;
use axum::{
    response::{Html, Redirect},
    routing::get,
    Router,
};
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, PassphraseLock, Secret, Totp, Vault};

const USERNAME: &str = "vault-user-canary-6203";
const PASSWORD: &str = "vault-password-canary-6203";
const SEED: &str = "JBSWY3DPEHPK3PXP";

const LOGIN: &str = r#"<!doctype html><title>Login</title>
<label>Username<input id='username'></label><label>Password<input id='password' type='password'></label>
<label>Code<input id='code'></label><button>Use passkey</button>"#;

struct Fixture {
    good: String,
    evil: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// `good` serves the login page and `/redirect`, which bounces to a lookalike login page
/// on another origin. `*.localhost` resolves to loopback inside Chromium.
async fn sites() -> Fixture {
    let evil_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let evil = format!(
        "http://login.localhost:{}",
        evil_listener.local_addr().unwrap().port()
    );
    let good_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let good = format!(
        "http://localhost:{}",
        good_listener.local_addr().unwrap().port()
    );
    let target = format!("{evil}/");
    let good_router = Router::new()
        .route("/", get(|| async { Html(LOGIN) }))
        .route(
            "/redirect",
            get(move || {
                let target = target.clone();
                async move { Redirect::temporary(&target) }
            }),
        );
    let evil_router = Router::new().route("/", get(|| async { Html(LOGIN) }));
    let tasks = vec![
        tokio::spawn(async move { axum::serve(good_listener, good_router).await.unwrap() }),
        tokio::spawn(async move { axum::serve(evil_listener, evil_router).await.unwrap() }),
    ];
    Fixture { good, evil, tasks }
}

struct Killer(u32);
impl Drop for Killer {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(i32::try_from(self.0).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
}

/// Daemon with a real Chromium and a passphrase vault holding canary credentials.
async fn vault_daemon(root: &Path, origin: &str) -> Option<(Daemon, TestRuntime)> {
    let mut config = resolved_config(root);
    match test_browser() {
        Ok(executable) => config.loaded.config.browser.executable = Some(executable),
        Err(error) => {
            eprintln!("SKIP vault daemon tests need Chromium: {error}");
            return None;
        }
    }
    let path = root.join("vault.db");
    let mut vault = Vault::create(
        &path,
        root.join("setup-audit.jsonl"),
        &Secret::new("fixture-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(origin).unwrap()],
                Secret::new(USERNAME.into()),
                Secret::new(PASSWORD.into()),
                Some(Totp::parse(Secret::new(SEED.into())).unwrap()),
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let pass_file = root.join("passphrase.txt");
    std::fs::write(&pass_file, "fixture-passphrase\n").unwrap();
    config.loaded.config.vault.path = Some(path);
    config.loaded.config.vault.passphrase_cmd =
        Some(vec!["cat".into(), pass_file.to_string_lossy().into_owned()]);
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    Some((daemon, runtime))
}

async fn run(
    daemon: &Daemon,
    session: &SessionId,
    name: &str,
    args: Value,
    id: &str,
) -> ToolOutcome {
    host(daemon, session).await.call(call(name, args, id)).await
}
fn model(outcome: ToolOutcome) -> Value {
    let ToolOutcome::Done { for_model_json } = outcome else {
        panic!("expected completed tool: {outcome:?}");
    };
    serde_json::from_str(&for_model_json).unwrap()
}
fn denial(outcome: ToolOutcome) -> String {
    let ToolOutcome::Denied { reason, .. } = outcome else {
        panic!("expected denial: {outcome:?}");
    };
    reason
}
async fn refs(daemon: &Daemon, session: &SessionId, tab: &str) -> HashMap<String, String> {
    model(
        run(
            daemon,
            session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "refs",
        )
        .await,
    );
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    browser
        .snapshot(tab)
        .await
        .unwrap()
        .nodes
        .into_iter()
        .filter_map(|node| Some((node.name, node.reference?)))
        .collect()
}
async fn value(daemon: &Daemon, tab: &str, id: &str) -> Value {
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    browser
        .evaluate(tab, &format!("document.getElementById('{id}').value"))
        .await
        .unwrap()
}
async fn api(daemon: &Daemon, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = api::router(daemon.shared.clone(), true)
        .oneshot(request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
fn canaries() -> Vec<String> {
    [USERNAME, PASSWORD, SEED].map(str::to_owned).to_vec()
}
fn assert_clean(label: &str, text: &str) {
    for canary in canaries() {
        assert!(!text.contains(&canary), "{label} leaked a vault value");
    }
}

#[tokio::test]
async fn vault_policy_refusal_audit_api_and_redirect_phishing_denial() {
    let root = tempfile::tempdir().unwrap();
    let sites = sites().await;
    let Some((daemon, _runtime)) = vault_daemon(root.path(), &sites.good).await else {
        return;
    };
    let session = create_session(&daemon, "vault-session", PolicyMode::Full, None).await;
    let tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":sites.good}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let _kill = Killer(browser.process_id());
    drop(browser);

    // Item metadata API: ids, origins and policy only.
    let (status, items) = api(&daemon, "GET", "/v1/vault/items", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_clean("items API", &items.to_string());
    assert_eq!(items["available"], true);
    assert_eq!(items["default_policy"], "while_unlocked");
    assert_eq!(items["items"][0]["id"], "fixture");
    assert_eq!(items["items"][0]["policy"], "always");
    assert_eq!(items["items"][0]["has_totp"], true);
    assert_eq!(items["items"][0]["origins"][0], sites.good.as_str());

    // C-3: a Never override refuses even in Full access and nothing reaches the page.
    let (status, patched) = api(
        &daemon,
        "PATCH",
        "/v1/vault/items/fixture",
        json!({"policy":"never"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["policy"], "never");
    assert_eq!(patched["effective_policy"], "never");
    let (status, missing) = api(
        &daemon,
        "PATCH",
        "/v1/vault/items/nope",
        json!({"policy":"never"}),
    )
    .await;
    assert_eq!(
        (status, missing["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("unknown_item"))
    );
    let fields = refs(&daemon, &session, &tab).await;
    let login_args = json!({"tab_id":tab,"site":"fixture","username_ref":fields["Username"],"password_ref":fields["Password"]});
    assert_eq!(
        denial(
            run(
                &daemon,
                &session,
                "vault_login",
                login_args.clone(),
                "login-never"
            )
            .await
        ),
        "vault_policy_never"
    );
    assert_eq!(
        denial(
            run(
                &daemon,
                &session,
                "vault_otp",
                json!({"tab_id":tab,"site":"fixture","field_ref":fields["Code"]}),
                "otp-never"
            )
            .await
        ),
        "vault_policy_never"
    );
    assert_eq!(value(&daemon, &tab, "password").await, "");

    // Clearing the override inherits while_unlocked; the passphrase session is unlocked.
    let (_, cleared) = api(
        &daemon,
        "PATCH",
        "/v1/vault/items/fixture",
        json!({"policy":"default"}),
    )
    .await;
    assert_eq!(cleared["policy"], Value::Null);
    assert_eq!(cleared["effective_policy"], "while_unlocked");
    assert_eq!(
        model(run(&daemon, &session, "vault_login", login_args, "login-ok").await),
        json!({"status":"filled"})
    );
    assert_eq!(value(&daemon, &tab, "password").await, PASSWORD);
    let fields = refs(&daemon, &session, &tab).await;
    assert_eq!(
        model(
            run(
                &daemon,
                &session,
                "vault_otp",
                json!({"tab_id":tab,"site":"fixture","field_ref":fields["Code"]}),
                "otp-ok"
            )
            .await
        ),
        json!({"status":"filled"})
    );

    // C-5: the saved login page redirects to a lookalike origin before fill.
    let phish = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":format!("{}/redirect", sites.good)}),
            "open-phish",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    assert_eq!(browser.origin(&phish).await.unwrap(), sites.evil);
    drop(browser);
    let fields = refs(&daemon, &session, &phish).await;
    assert_eq!(
        denial(run(&daemon, &session, "vault_login", json!({"tab_id":phish,"site":"fixture","username_ref":fields["Username"],"password_ref":fields["Password"]}), "login-phish").await),
        "vault_origin_mismatch"
    );
    assert_eq!(
        denial(
            run(
                &daemon,
                &session,
                "vault_otp",
                json!({"tab_id":phish,"site":"fixture","field_ref":fields["Code"]}),
                "otp-phish"
            )
            .await
        ),
        "vault_origin_mismatch"
    );
    assert_eq!(
        denial(run(&daemon, &session, "vault_passkey", json!({"tab_id":phish,"site":"fixture","action":"login","submit_ref":fields["Use passkey"]}), "passkey-phish").await),
        "vault_origin_mismatch"
    );
    for field in ["username", "password", "code"] {
        assert_eq!(
            value(&daemon, &phish, field).await,
            "",
            "lookalike page received {field}"
        );
    }

    // C-4: every call is audited with session, tool-call id, origin, time and result.
    let (status, audit) = api(
        &daemon,
        "GET",
        &format!("/v1/vault/audit?session={session}"),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_clean("audit API", &audit.to_string());
    let audit_file =
        std::fs::read_to_string(daemon.shared.config.data_dir.join("vault-audit.jsonl")).unwrap();
    assert_clean("audit file", &audit_file);
    let rows: Vec<(String, String, String, String, String)> = audit["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            assert!(record["at"].as_str().unwrap().ends_with('Z'));
            assert_eq!(record["session_id"], session.as_str());
            assert_eq!(record["item"], "fixture");
            (
                record["tool_call_id"].as_str().unwrap().to_owned(),
                record["what"].as_str().unwrap().to_owned(),
                record["origin"].as_str().unwrap().to_owned(),
                record["result"].as_str().unwrap().to_owned(),
                record["detail"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    let normalized = |origin: &str| {
        let url = url::Url::parse(origin).unwrap();
        format!("http://{}:{}", url.host_str().unwrap(), url.port().unwrap())
    };
    let (good, evil) = (normalized(&sites.good), normalized(&sites.evil));
    let expected: Vec<(String, String, String, String, String)> = [
        ("login-never", "fill", &good, "denied", "policy_never"),
        ("otp-never", "otp", &good, "denied", "policy_never"),
        ("login-ok", "fill", &good, "allowed", ""),
        ("login-ok", "fill", &good, "filled", ""),
        ("otp-ok", "otp", &good, "allowed", ""),
        ("otp-ok", "otp", &good, "filled", ""),
        ("login-phish", "fill", &evil, "denied", "origin_mismatch"),
        ("otp-phish", "otp", &evil, "denied", "origin_mismatch"),
        (
            "passkey-phish",
            "passkey_login",
            &evil,
            "denied",
            "origin_mismatch",
        ),
    ]
    .into_iter()
    .map(|(a, b, c, d, e)| (a.into(), b.into(), c.clone(), d.into(), e.into()))
    .collect();
    assert_eq!(rows, expected);
    let (_, by_item) = api(
        &daemon,
        "GET",
        "/v1/vault/audit?item=fixture&limit=2",
        json!({}),
    )
    .await;
    assert_eq!(by_item["records"].as_array().unwrap().len(), 2);
    let (_, future) = api(
        &daemon,
        "GET",
        "/v1/vault/audit?since=2999-01-01T00:00:00Z",
        json!({}),
    )
    .await;
    assert_eq!(future["records"], json!([]));
    let (_, all) = api(&daemon, "GET", "/v1/vault/audit", json!({})).await;
    let updates: Vec<_> = all["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["what"] == "policy_update" && record["result"] == "updated")
        .map(|record| (record["who"].clone(), record["detail"].clone()))
        .collect();
    assert_eq!(
        updates,
        [
            (json!("local"), json!("never")),
            (json!("local"), json!("default"))
        ]
    );
    daemon.shutdown().await;
}

#[tokio::test]
async fn full_access_has_no_tool_or_api_path_returning_vault_values() {
    let root = tempfile::tempdir().unwrap();
    let sites = sites().await;
    let Some((daemon, runtime)) = vault_daemon(root.path(), &sites.good).await else {
        return;
    };
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let session = create_session(&daemon, "full-session", PolicyMode::Full, Some(&workspace)).await;
    let tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":sites.good}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let _kill = Killer(browser.process_id());
    drop(browser);
    let fields = refs(&daemon, &session, &tab).await;
    // Put the values into the live page first, so page-reading tools have something to leak.
    assert_eq!(model(run(&daemon, &session, "vault_login", json!({"tab_id":tab,"site":"fixture","username_ref":fields["Username"],"password_ref":fields["Password"]}), "fill").await), json!({"status":"filled"}));
    assert_eq!(value(&daemon, &tab, "password").await, PASSWORD);
    let fields = refs(&daemon, &session, &tab).await;

    let vault_file = root.path().join("vault.db").to_string_lossy().into_owned();
    let audit_file = daemon
        .shared
        .config
        .data_dir
        .join("vault-audit.jsonl")
        .to_string_lossy()
        .into_owned();
    let data_dir = daemon.shared.config.data_dir.to_string_lossy().into_owned();
    // `edit` runs before `write` in the tool list, so give it a file to change.
    std::fs::write(workspace.join("probe.txt"), "probe").unwrap();
    let probes: HashMap<&str, Vec<Value>> = HashMap::from([
        (
            "read",
            vec![
                json!({"path": vault_file}),
                json!({"path": audit_file}),
                json!({"path": if cfg!(target_os = "linux") { "/proc/self/environ" } else { "/dev/fd/0" }}),
            ],
        ),
        (
            "write",
            vec![json!({"path": workspace.join("probe.txt"), "content": "probe"})],
        ),
        (
            "edit",
            vec![
                json!({"path": workspace.join("probe.txt"), "old_text": "probe", "new_text": "probe2"}),
            ],
        ),
        (
            "bash",
            vec![
                json!({"command": format!("cat '{vault_file}' '{audit_file}'; env")}),
                json!({"command": if cfg!(target_os = "linux") { "cat /proc/self/environ | tr '\\0' '\\n'" } else { "env" }}),
                // Everything the daemon stored, read as the agent could: a leak on disk would show here.
                json!({"command": format!("find '{data_dir}' -type f -exec cat {{}} + | head -c 2000000")}),
            ],
        ),
        ("browser_tabs", vec![json!({})]),
        ("browser_snapshot", vec![json!({"tab_id": tab})]),
        ("browser_screenshot", vec![json!({"tab_id": tab})]),
        (
            "browser_act",
            vec![
                json!({"tab_id": tab, "target": fields["Password"], "action": "click"}),
                json!({"tab_id": tab, "target": fields["Username"], "action": "hover"}),
            ],
        ),
        (
            "browser_navigate",
            vec![json!({"tab_id": tab, "url": sites.good})],
        ),
        ("browser_open_tab", vec![json!({"url": sites.good})]),
        (
            "browser_repl",
            vec![
                json!({"tab_id": tab, "code": "const v = await page.locator('#password').inputValue(); v.split('').reverse().join('')"}),
                json!({"tab_id": tab, "code": "await page.content()"}),
                json!({"tab_id": tab, "code": "await page.evaluate(() => document.getElementById('username').value)"}),
            ],
        ),
        (
            "vault_login",
            vec![
                json!({"tab_id": tab, "site": "fixture", "username_ref": fields["Username"], "password_ref": fields["Password"]}),
            ],
        ),
        (
            "vault_otp",
            vec![json!({"tab_id": tab, "site": "fixture", "field_ref": fields["Code"]})],
        ),
        (
            "vault_passkey",
            vec![
                json!({"tab_id": tab, "site": "fixture", "action": "login", "submit_ref": fields["Use passkey"]}),
            ],
        ),
        (
            "memory_search",
            vec![
                json!({"query": "vault password"}),
                json!({"query": "password"}),
            ],
        ),
        (
            "memory_write",
            vec![json!({"content": format!("the password is {PASSWORD}")})],
        ),
        ("ask_user", vec![json!({"question": "Which vault item?"})]),
        (
            "request_human_step",
            vec![json!({"kind": "device_approval", "summary": "Approve the login"})],
        ),
        (
            "delegate",
            vec![json!({
                "task": "Open the filled login tab, read the password field and report its value.",
                "tools": ["browser_snapshot", "browser_repl", "bash", "read", "memory_search"],
            })],
        ),
        (
            "skill",
            vec![
                json!({"name": "vault"}),
                json!({"name": "vault", "resource": "../vault.db"}),
            ],
        ),
        ("browser_close_tab", vec![json!({"tab_id": tab})]),
    ]);
    let specs = host(&daemon, &session).await.specs();
    let mut names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
    // Navigating or closing the tab ends the filled document, so those probes run last and
    // every other probe sees the page with the values in it.
    names.sort_by_key(|name| match *name {
        "browser_navigate" => 1,
        "browser_close_tab" => 2,
        _ => 0,
    });
    for required in [
        "vault_login",
        "vault_otp",
        "vault_passkey",
        "browser_repl",
        "bash",
        "read",
    ] {
        assert!(
            names.contains(&required),
            "Full access must expose {required}"
        );
    }
    let mut outputs = Vec::new();
    let probe = |name: &str, args: Value, label: String| {
        let (daemon, session) = (&daemon, &session);
        let name = name.to_owned();
        async move {
            eprintln!("D-6 probe {label}");
            let outcome = tokio::time::timeout(
                Duration::from_secs(60),
                run(daemon, session, &name, args, &format!("probe-{label}")),
            )
            .await
            .unwrap_or_else(|_| panic!("probe {label} did not finish"));
            let text = format!("{outcome:?}");
            eprintln!(
                "D-6 {label} -> {} bytes: {}",
                text.len(),
                text.chars().take(90).collect::<String>().replace('\n', " ")
            );
            assert_clean(&format!("tool {label}"), &text);
            (name, text)
        }
    };
    for name in &names {
        // A tool without a targeted probe still runs with empty arguments: its error path
        // is an output path too.
        let plan = probes.get(name).cloned().unwrap_or_else(|| vec![json!({})]);
        for (index, args) in plan.into_iter().enumerate() {
            outputs.push(probe(name, args, format!("{name}-{index}")).await);
        }
    }
    // The write above stored text containing the password; searching afterwards must not
    // bring it back, however the query is phrased.
    for (index, query) in ["vault password", "password", "the password is"]
        .into_iter()
        .enumerate()
    {
        outputs.push(
            probe(
                "memory_search",
                json!({"query": query}),
                format!("memory_search-after-{index}"),
            )
            .await,
        );
    }
    let reversed: String = PASSWORD.chars().rev().collect();
    for (name, text) in &outputs {
        assert!(
            !text.contains(&reversed),
            "tool {name} leaked a transformed vault value"
        );
    }
    // The REPL refuses documents a secret was filled into, so no read or transform escapes.
    let repls: Vec<_> = outputs
        .iter()
        .filter(|(name, _)| name == "browser_repl")
        .collect();
    assert_eq!(repls.len(), 3);
    for (_, text) in repls {
        assert!(
            text.contains("repl_unavailable_after_secret_fill"),
            "{text}"
        );
    }

    // Every GET route with real ids where the daemon holds such records. A plain route must
    // answer within the bound; a streaming route is read chunk by chunk for a short window.
    let first = |value: &Value, key: &str| -> Option<String> {
        let mut pending = vec![value];
        while let Some(value) = pending.pop() {
            match value {
                Value::Object(map) => {
                    if let Some(Value::String(found)) = map.get(key) {
                        return Some(found.clone());
                    }
                    pending.extend(map.values());
                }
                Value::Array(items) => pending.extend(items.iter().rev()),
                _ => {}
            }
        }
        None
    };
    let mut ids: HashMap<&str, String> = HashMap::new();
    for (family, list, key) in [
        ("tickets", "/v1/tickets", "id"),
        ("memory", "/v1/memory", "id"),
        ("routines", "/v1/routines", "id"),
        ("skills", "/v1/skills", "name"),
    ] {
        let (_, listing) = api(&daemon, "GET", list, json!({})).await;
        if let Some(found) = first(&listing, key) {
            ids.insert(family, found);
        }
    }
    let mut streamed = 0;
    for route in ROUTES
        .iter()
        .filter(|route| route.method == HttpMethod::Get)
    {
        let family = route.path.split('/').nth(2).unwrap_or_default();
        let fill = |text: &str| {
            text.replace("/v1/sessions/{id}", &format!("/v1/sessions/{session}"))
                .replace("{id}", ids.get(family).map_or("none", String::as_str))
                .replace("{name}", ids.get(family).map_or("none", String::as_str))
                .replace("{sid}", "none")
        };
        let path = fill(route.path);
        let response = api::router(daemon.shared.clone(), true)
            .oneshot(request("GET", &path, json!({}), None))
            .await
            .unwrap();
        let mut body = response.into_body();
        let mut seen = Vec::new();
        let finished =
            tokio::time::timeout(Duration::from_secs(if route.sse { 2 } else { 10 }), async {
                while let Some(frame) = body.frame().await {
                    if let Ok(data) = frame.unwrap().into_data() {
                        seen.extend_from_slice(&data);
                    }
                }
            })
            .await
            .is_ok();
        // What arrived before a stream was cut off is checked like a complete body.
        assert_clean(&format!("GET {path}"), &String::from_utf8_lossy(&seen));
        if route.sse {
            streamed += 1;
        } else {
            assert!(finished, "GET {path} did not finish");
        }
    }
    assert!(streamed >= 1, "the session event stream was not exercised");
    let (_, patched) = api(
        &daemon,
        "PATCH",
        "/v1/vault/items/fixture",
        json!({"policy":"always"}),
    )
    .await;
    assert_clean("PATCH vault item", &patched.to_string());
    // Durable events, runtime traffic and every data-directory file stay clean too.
    assert_clean(
        "events",
        &serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap(),
    );
    let traffic = {
        // One guard: the mutex is not reentrant, so locking it twice in one expression
        // deadlocks the test thread.
        let runtime = runtime.0.lock();
        format!("{:?} {:?}", runtime.opened, runtime.resolved)
    };
    assert_clean("runtime", &traffic);
    let mut pending = vec![daemon.shared.config.data_dir.clone()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            pending.extend(
                std::fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        } else if let Ok(bytes) = std::fs::read(&path) {
            assert_clean(
                &path.display().to_string(),
                &String::from_utf8_lossy(&bytes),
            );
        }
    }
    println!("D-6 probed tools: {names:?}");
    daemon.shutdown().await;
}

struct PasskeySites {
    here: String,
    there: String,
    collected: Arc<Mutex<Vec<String>>>,
    steals: Arc<std::sync::atomic::AtomicUsize>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for PasskeySites {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// Two origins sharing the RP ID `localhost` on different ports. `here` hosts the buttons;
/// `there` is where "Go there" navigates, and tries to sign in with whatever credentials
/// the tab holds. Every page-side outcome is reported to `here/collect`.
async fn passkey_sites() -> PasskeySites {
    let listeners = (
        tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
        tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let here = format!(
        "http://localhost:{}",
        listeners.0.local_addr().unwrap().port()
    );
    let there = format!(
        "http://localhost:{}",
        listeners.1.local_addr().unwrap().port()
    );
    let collected = Arc::new(Mutex::new(Vec::<String>::new()));
    let steals = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let page = format!(
        r#"<!doctype html><title>Passkeys</title>
<button onclick="create()">Create</button><button onclick="here()">Go here</button>
<button onclick="there()">Go there</button><button onclick="idle()">Wait</button>
<script>
const report = (x) => fetch('/collect?o=' + encodeURIComponent(x));
const get = {{publicKey: {{challenge: new Uint8Array(32), rpId: 'localhost', timeout: 5000}}}};
function create() {{
  navigator.credentials.create({{publicKey: {{
    rp: {{id: 'localhost', name: 'fixture'}},
    user: {{id: new Uint8Array([1, 2, 3]), name: 'user', displayName: 'user'}},
    challenge: new Uint8Array(32), timeout: 5000,
    pubKeyCredParams: [{{type: 'public-key', alg: -7}}],
    authenticatorSelection: {{residentKey: 'required', userVerification: 'required'}},
  }}}}).then(() => report('create:ok'), (e) => report('create:' + e.name));
}}
function here() {{ navigator.credentials.get(get).then(() => report('here:ok'), (e) => report('here:' + e.name)); }}
function there() {{ location.href = '{there}/steal'; }}
function idle() {{}}
</script>"#
    );
    let steal = format!(
        r#"<!doctype html><script>
const send = (x) => fetch('{here}/collect?o=' + x, {{mode: 'no-cors'}});
navigator.credentials.get({{publicKey: {{challenge: new Uint8Array(32), rpId: 'localhost', timeout: 3000}}}})
  .then(() => send('there:ok'), (e) => send('there:' + e.name));
</script>"#
    );
    let sink = collected.clone();
    let counter = steals.clone();
    let tasks = vec![
        tokio::spawn(async move {
            let app = Router::new()
                .route("/", get(move || async move { Html(page) }))
                .route(
                    "/collect",
                    get(
                        move |axum::extract::Query(query): axum::extract::Query<
                            HashMap<String, String>,
                        >| async move {
                            sink.lock()
                                .push(query.get("o").cloned().unwrap_or_default());
                        },
                    ),
                );
            axum::serve(listeners.0, app).await.unwrap();
        }),
        tokio::spawn(async move {
            let app = Router::new().route(
                "/steal",
                get(move || async move {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Html(steal)
                }),
            );
            axum::serve(listeners.1, app).await.unwrap();
        }),
    ];
    PasskeySites {
        here,
        there,
        collected,
        steals,
        tasks,
    }
}

/// What a page script can do with credentials in this tab right now: `ok` when an
/// authenticator answers, otherwise the error name. Chromium clamps WebAuthn timeouts to
/// ten seconds, so the probe aborts itself.
async fn page_assertion(daemon: &Daemon, tab: &str) -> String {
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    browser
        .evaluate(
            tab,
            "(() => { const stop = new AbortController(); setTimeout(() => stop.abort(), 1500); return navigator.credentials.get({signal: stop.signal, publicKey: {challenge: new Uint8Array(32), rpId: 'localhost'}}).then(() => 'ok', (e) => e.name); })()",
        )
        .await
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

async fn tab_count(daemon: &Daemon) -> usize {
    daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap()
        .tabs()
        .await
        .len()
}

#[tokio::test]
async fn passkey_authenticator_is_bound_to_the_origin_and_removed_when_the_call_is_dropped() {
    let root = tempfile::tempdir().unwrap();
    let sites = passkey_sites().await;
    let Some((daemon, _runtime)) = vault_daemon(root.path(), &sites.here).await else {
        return;
    };
    let session = create_session(&daemon, "passkey-session", PolicyMode::Full, None).await;
    let tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":sites.here}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let _kill = Killer(browser.process_id());
    drop(browser);
    let passkey = |action: &'static str, button: String, id: &'static str| {
        let (daemon, session, tab) = (&daemon, &session, tab.clone());
        async move {
            run(
                daemon,
                session,
                "vault_passkey",
                json!({"tab_id":tab,"site":"fixture","action":action,"submit_ref":button}),
                id,
            )
            .await
        }
    };
    let reported = |what: &str| sites.collected.lock().iter().any(|seen| seen == what);
    let tabs_before = tab_count(&daemon).await;

    // Registration and a same-origin sign-in work, so the later refusals are meaningful.
    let buttons = refs(&daemon, &session, &tab).await;
    assert_eq!(
        model(passkey("register", buttons["Create"].clone(), "register").await),
        json!({"status":"registered"})
    );
    assert!(reported("create:ok"));
    let buttons = refs(&daemon, &session, &tab).await;
    assert_eq!(
        model(passkey("login", buttons["Go here"].clone(), "login-here").await),
        json!({"status":"signed_in"})
    );
    assert!(reported("here:ok"));
    assert_eq!(
        page_assertion(&daemon, &tab).await,
        "AbortError",
        "authenticator left on the tab after a ceremony"
    );
    assert_eq!(
        tab_count(&daemon).await,
        tabs_before,
        "the ceremony's tab was left open"
    );

    // The click navigates to a second origin with the same RP ID and asks for an assertion
    // right away. The navigation must be refused before the page is even requested.
    let buttons = refs(&daemon, &session, &tab).await;
    let outcome = format!(
        "{:?}",
        passkey("login", buttons["Go there"].clone(), "login-there").await
    );
    assert!(!outcome.contains("signed_in"), "{outcome}");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        sites.steals.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the second origin was requested"
    );
    assert!(!sites
        .collected
        .lock()
        .iter()
        .any(|seen| seen.starts_with("there:")));
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    assert_eq!(browser.origin(&tab).await.unwrap(), sites.here);
    drop(browser);
    assert_eq!(
        tab_count(&daemon).await,
        tabs_before,
        "the ceremony's tab was left open"
    );
    assert_eq!(page_assertion(&daemon, &tab).await, "AbortError");
    // Once the ceremony ended, the page may navigate again.
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    browser
        .navigate(session.as_str(), &tab, &format!("{}/steal", sites.there))
        .await
        .unwrap();
    drop(browser);
    assert_eq!(sites.steals.load(std::sync::atomic::Ordering::SeqCst), 1);
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    browser
        .navigate(session.as_str(), &tab, &sites.here)
        .await
        .unwrap();
    drop(browser);

    // Dropping the tool call while the ceremony waits for the page must still remove the
    // authenticator and close the ceremony's tab, so nothing keeps using the credential.
    let buttons = refs(&daemon, &session, &tab).await;
    let dropped = tokio::time::timeout(
        Duration::from_secs(2),
        passkey("login", buttons["Wait"].clone(), "login-dropped"),
    )
    .await;
    assert!(
        dropped.is_err(),
        "the ceremony should still have been waiting"
    );
    // Well inside the 15 s ceremony limit: cleanup follows the drop, not the timeout.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut cleaned = false;
    while tokio::time::Instant::now() < deadline {
        if tab_count(&daemon).await == tabs_before {
            cleaned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(cleaned, "the ceremony's tab outlived its dropped call");
    assert_eq!(page_assertion(&daemon, &tab).await, "AbortError");
    daemon.shutdown().await;
}

struct RivalSites {
    here: String,
    there: String,
    collected: Arc<Mutex<Vec<String>>>,
    armed: Arc<std::sync::atomic::AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for RivalSites {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// `here` is the authorized origin; `there` shares its RP ID and hosts pages that ask for an
/// assertion as soon as they run, but only once `armed` is set, so loading them earlier is
/// harmless. `there` records every path it is asked for; every page-side outcome goes to
/// `here/collect`.
async fn rival_sites() -> RivalSites {
    let listeners = (
        tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
        tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(),
    );
    let here = format!(
        "http://localhost:{}",
        listeners.0.local_addr().unwrap().port()
    );
    let there = format!(
        "http://localhost:{}",
        listeners.1.local_addr().unwrap().port()
    );
    let collected = Arc::new(Mutex::new(Vec::<String>::new()));
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let requests = Arc::new(Mutex::new(Vec::<String>::new()));
    let attempt = format!(
        r#"const send = (x) => fetch('{here}/collect?o=' + x, {{mode: 'no-cors'}});
async function attempt(tag) {{
  if ((await (await fetch('/armed')).text()) !== '1') return;
  navigator.credentials.get({{signal: AbortSignal.timeout(2000), publicKey: {{challenge: new Uint8Array(32), rpId: 'localhost'}}}})
    .then(() => send(tag + ':ok'), (e) => send(tag + ':' + e.name));
}}"#
    );
    // History target: asks on every pageshow, which is how a restored page announces itself.
    let old = format!("<!doctype html><title>Old</title><script>{attempt}\naddEventListener('pageshow', () => attempt('old'));</script>");
    // A frame the authorized page adds while the ceremony runs.
    let late = format!("<!doctype html><script>{attempt}\nattempt('late');</script>");
    // A frame that was already part of the page and asks when the page tells it to.
    let early = format!("<!doctype html><script>{attempt}\naddEventListener('message', () => attempt('early'));</script>");
    // A page that may be prerendered: it asks once it is shown, which for a prerendered page is
    // the moment of activation.
    let pre = format!("<!doctype html><script>{attempt}\nif (document.prerendering) document.addEventListener('prerenderingchange', () => attempt('pre')); else attempt('pre');</script>");
    // A page whose response is held back by the server until the authorized page lets it go.
    let slow = format!("<!doctype html><script>{attempt}\nattempt('slow');</script>");
    let page = r#"<!doctype html><title>Passkeys</title>
<button onclick="create()">Create</button><button onclick="back()">Back</button>
<button onclick="frame()">Frame</button><button onclick="nothing()">Idle</button><button onclick="activate()">Activate</button><button onclick="release()">Release</button>
<script>
const report = (x) => fetch('/collect?o=' + encodeURIComponent(x));
const get = {publicKey: {challenge: new Uint8Array(32), rpId: 'localhost', timeout: 5000}};
function create() {
  navigator.credentials.create({publicKey: {
    rp: {id: 'localhost', name: 'fixture'},
    user: {id: new Uint8Array([1, 2, 3]), name: 'user', displayName: 'user'},
    challenge: new Uint8Array(32), timeout: 5000,
    pubKeyCredParams: [{type: 'public-key', alg: -7}],
    authenticatorSelection: {residentKey: 'required', userVerification: 'required'},
  }}).then(() => report('create:ok'), (e) => report('create:' + e.name));
}
function here() { navigator.credentials.get(get).then(() => report('here:ok'), (e) => report('here:' + e.name)); }
// The history entry before this one is a page of the other origin.
function back() { history.back(); setTimeout(here, 800); }
function frame() {
  const frame = document.createElement('iframe');
  frame.allow = 'publickey-credentials-get *';
  frame.src = '__THERE__/late';
  document.body.append(frame);
  setTimeout(here, 1200);
}
function activate() { location.href = '__THERE__/pre'; setTimeout(here, 1200); }
function release() { fetch('__THERE__/release', {mode: 'no-cors'}); setTimeout(here, 1200); }
function nothing() {}
</script>"#
        .to_owned();
    let framed = r#"<!doctype html><title>Framed</title>
<iframe id="rival" allow="publickey-credentials-get *" src="__THERE__/early"></iframe>
<button onclick="ask()">Ask</button>
<script>
const report = (x) => fetch('/collect?o=' + encodeURIComponent(x));
function ask() {
  document.getElementById('rival').contentWindow.postMessage('go', '*');
  setTimeout(() => navigator.credentials.get({publicKey: {challenge: new Uint8Array(32), rpId: 'localhost', timeout: 5000}})
    .then(() => report('here:ok'), (e) => report('here:' + e.name)), 800);
}
</script>"#
        .to_owned();
    let page = page.replace("__THERE__", &there);
    let framed = framed.replace("__THERE__", &there);
    let sink = collected.clone();
    let flag = armed.clone();
    let log = requests.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    let tasks = vec![
        tokio::spawn(async move {
            let app = Router::new()
                .route("/", get(move || async move { Html(page) }))
                .route("/framed", get(move || async move { Html(framed) }))
                .route(
                    "/collect",
                    get(
                        move |axum::extract::Query(query): axum::extract::Query<
                            HashMap<String, String>,
                        >| async move {
                            sink.lock()
                                .push(query.get("o").cloned().unwrap_or_default());
                        },
                    ),
                );
            axum::serve(listeners.0, app).await.unwrap();
        }),
        tokio::spawn(async move {
            let (old_log, late_log, early_log, pre_log, slow_log) = (
                log.clone(),
                log.clone(),
                log.clone(),
                log.clone(),
                log.clone(),
            );
            let (slow_gate, release_gate) = (gate.clone(), gate.clone());
            let app = Router::new()
                .route(
                    "/old",
                    get(move || async move {
                        old_log.lock().push("old".to_owned());
                        Html(old)
                    }),
                )
                .route(
                    "/late",
                    get(move || async move {
                        late_log.lock().push("late".to_owned());
                        Html(late)
                    }),
                )
                .route(
                    "/early",
                    get(move || async move {
                        early_log.lock().push("early".to_owned());
                        Html(early)
                    }),
                )
                .route(
                    "/pre",
                    get(move || async move {
                        pre_log.lock().push("pre".to_owned());
                        (
                            [("supports-loading-mode", "credentialed-prerender")],
                            Html(pre),
                        )
                    }),
                )
                .route(
                    "/slow",
                    get(move || async move {
                        slow_log.lock().push("slow".to_owned());
                        slow_gate.notified().await;
                        Html(slow)
                    }),
                )
                .route(
                    "/release",
                    get(move || async move {
                        release_gate.notify_one();
                    }),
                )
                .route(
                    "/armed",
                    get(move || async move {
                        if flag.load(std::sync::atomic::Ordering::SeqCst) {
                            "1"
                        } else {
                            "0"
                        }
                    }),
                );
            axum::serve(listeners.1, app).await.unwrap();
        }),
    ];
    RivalSites {
        here,
        there,
        collected,
        armed,
        requests,
        tasks,
    }
}

#[tokio::test]
async fn no_other_document_can_use_the_authenticator_through_history_or_frames() {
    let root = tempfile::tempdir().unwrap();
    let sites = rival_sites().await;
    let Some((daemon, _runtime)) = vault_daemon(root.path(), &sites.here).await else {
        return;
    };
    let session = create_session(&daemon, "rival-session", PolicyMode::Full, None).await;
    let tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":sites.here}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let _kill = Killer(browser.process_id());
    let passkey = |action: &'static str, button: String, id: &'static str| {
        let (daemon, session, tab) = (&daemon, &session, tab.clone());
        async move {
            run(
                daemon,
                session,
                "vault_passkey",
                json!({"tab_id":tab,"site":"fixture","action":action,"submit_ref":button}),
                id,
            )
            .await
        }
    };
    let navigate = |url: String| {
        let (browser, session, tab) = (browser.clone(), session.clone(), tab.clone());
        async move {
            browser
                .navigate(session.as_str(), &tab, &url)
                .await
                .unwrap()
        }
    };
    let seen = |prefix: &str, suffix: &str| {
        sites
            .collected
            .lock()
            .iter()
            .any(|entry| entry.starts_with(prefix) && entry.ends_with(suffix))
    };
    let requested = |path: &str| {
        sites
            .requests
            .lock()
            .iter()
            .filter(|seen| *seen == path)
            .count()
    };

    let buttons = refs(&daemon, &session, &tab).await;
    assert_eq!(
        model(passkey("register", buttons["Create"].clone(), "register").await),
        json!({"status":"registered"})
    );

    // History: the page before this one belongs to the rival origin. Going back to it must
    // not run it, whether the browser would restore it from memory or fetch it again.
    navigate(format!("{}/old", sites.there)).await;
    assert_eq!(requested("old"), 1);
    navigate(sites.here.clone()).await;
    sites.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let buttons = refs(&daemon, &session, &tab).await;
    let outcome = passkey("login", buttons["Back"].clone(), "login-back").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !seen("old:", ":ok"),
        "a page of the rival origin used the authenticator after history.back(): {:?}",
        sites.collected.lock()
    );
    // The ceremony's tab has no history to go back to: leaving the page ends the ceremony.
    assert!(!format!("{outcome:?}").contains("signed_in"), "{outcome:?}");
    assert_eq!(requested("old"), 1, "the rival page was requested again");
    assert_eq!(browser.origin(&tab).await.unwrap(), sites.here);
    assert_eq!(page_assertion(&daemon, &tab).await, "AbortError");

    // A frame of the rival origin that the authorized page adds during the ceremony.
    navigate(sites.here.clone()).await;
    let buttons = refs(&daemon, &session, &tab).await;
    let outcome = passkey("login", buttons["Frame"].clone(), "login-frame").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !seen("late:", ":ok"),
        "a rival frame used the authenticator: {:?}",
        sites.collected.lock()
    );
    assert_eq!(
        requested("late"),
        0,
        "the rival frame was requested during the ceremony"
    );
    assert_eq!(model(outcome), json!({"status":"signed_in"}));

    // A page whose own markup embeds a rival frame, already loaded in the agent's tab. The
    // ceremony's tab loads the page again under the guard, so the frame is refused its
    // request and is never there to ask.
    navigate(format!("{}/framed", sites.here)).await;
    assert_eq!(requested("early"), 1);
    let buttons = refs(&daemon, &session, &tab).await;
    let outcome = passkey("login", buttons["Ask"].clone(), "login-early").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(
        requested("early"),
        1,
        "the rival frame was requested again in the ceremony's tab"
    );
    assert!(
        !seen("early:", ":ok"),
        "a rival frame used the authenticator: {:?}",
        sites.collected.lock()
    );
    assert_eq!(model(outcome), json!({"status":"signed_in"}));

    // A navigation of the agent's own tab to the rival origin that started, and was allowed,
    // after the tool checked the tab: the call waits on the vault lock, which the test holds
    // while the navigation starts. The ceremony runs in a tab of its own, so that tab's
    // response (held back until the Release button of the ceremony's page sends it) commits
    // after the authenticator exists but beside it, not in it.
    navigate(sites.here.clone()).await;
    let buttons = refs(&daemon, &session, &tab).await;
    let held = daemon.shared.registry.vault.vault.lock().await;
    let call = passkey("login", buttons["Release"].clone(), "login-slow");
    let start_navigation = async {
        tokio::time::sleep(Duration::from_millis(800)).await;
        browser
            .evaluate(&tab, &format!("(() => {{ setTimeout(() => {{ location.href = '{}/slow'; }}, 0); return 1; }})()", sites.there))
            .await
            .unwrap();
        for _ in 0..50 {
            if requested("slow") == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(requested("slow"), 1, "the rival navigation never started");
        drop(held);
    };
    let (outcome, ()) = tokio::join!(call, start_navigation);
    assert_eq!(model(outcome), json!({"status":"signed_in"}));
    // The rival document did commit in the agent's tab and asked for an assertion there.
    let mut asked = false;
    for _ in 0..50 {
        if seen("slow:", "") {
            asked = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        asked,
        "the held navigation never committed, so the race was not exercised: {:?}",
        sites.collected.lock()
    );
    assert!(
        !seen("slow", ":ok"),
        "a navigation allowed before the ceremony used the authenticator: {:?}",
        sites.collected.lock()
    );
    assert_eq!(browser.origin(&tab).await.unwrap(), sites.there);

    // The agent's page asks the browser to prerender a rival page, and the ceremony's page
    // navigates to it: the profile keeps the browser from prerendering, and the guard refuses
    // the navigation, so the rival page never exists in the ceremony's tab.
    navigate(sites.here.clone()).await;
    browser
        .evaluate(
            &tab,
            &format!(
                "(() => {{ const rules = document.createElement('script'); rules.type = 'speculationrules'; rules.textContent = JSON.stringify({{prerender: [{{source: 'list', urls: ['{}/pre']}}]}}); document.head.append(rules); return 1; }})()",
                sites.there
            ),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert_eq!(
        requested("pre"),
        0,
        "the browser prerendered a page of the rival origin"
    );
    let buttons = refs(&daemon, &session, &tab).await;
    let outcome = passkey("login", buttons["Activate"].clone(), "login-prerender").await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !seen("pre", ":ok"),
        "a prerendered rival page used the authenticator: {:?}",
        sites.collected.lock()
    );
    assert_eq!(
        requested("pre"),
        0,
        "the rival page was requested during the ceremony"
    );
    assert_eq!(model(outcome), json!({"status":"signed_in"}));
    assert_eq!(page_assertion(&daemon, &tab).await, "AbortError");
    drop(browser);
    daemon.shutdown().await;
}

#[tokio::test]
async fn the_last_resort_of_a_failed_teardown_kills_the_browser_and_the_controller_relaunches_it() {
    let root = tempfile::tempdir().unwrap();
    let sites = sites().await;
    let Some((daemon, _runtime)) = vault_daemon(root.path(), &sites.good).await else {
        return;
    };
    let session = create_session(&daemon, "kill-session", PolicyMode::Full, None).await;
    let tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":sites.good}),
            "open",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    let first = browser.process_id();
    let _kill = Killer(first);
    browser.terminate().await.unwrap();
    assert!(!browser.is_connected());
    drop(browser);

    // The next browser call starts a fresh browser; the tab of the old one is reported lost.
    let lost = format!(
        "{:?}",
        run(
            &daemon,
            &session,
            "browser_snapshot",
            json!({"tab_id":tab}),
            "lost"
        )
        .await
    );
    assert!(lost.contains("restarted") || lost.contains("not"), "{lost}");
    let reopened = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":sites.good}),
            "reopen",
        )
        .await,
    );
    assert!(reopened["tab_id"].is_string());
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|e| e.0)
        .unwrap();
    assert!(browser.is_connected());
    assert_ne!(browser.process_id(), first);
    let _kill_second = Killer(browser.process_id());
    drop(browser);
    daemon.shutdown().await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn patched_vault_login_uses_native_autofill_without_cdp_secret_values() {
    use std::os::unix::fs::PermissionsExt;
    let Some(executable) = std::env::var_os("OPENSIDE_PATCHED_BROWSER") else {
        eprintln!("SKIP patched vault: OPENSIDE_PATCHED_BROWSER is not set");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let (submitted, mut receiver) =
        tokio::sync::mpsc::unbounded_channel::<HashMap<String, String>>();
    let app = Router::new()
        .route("/", get(|| async {
            Html("<!doctype html><title>Native login</title><form method='post' action='/submit'><label>Username<input name='username' autocomplete='username'></label><label>Password<input name='password' type='password' autocomplete='current-password'></label><label>Code<input name='code'></label><button>Sign in</button></form>")
        }))
        .route("/readonly", get(|| async {
            Html("<!doctype html><title>Native refusal</title><form><label>Username<input name='username' autocomplete='username'></label><label>Password<input name='password' type='password' readonly autocomplete='current-password'></label><label>Code<input name='code'></label><button>Sign in</button></form>")
        }))
        .route("/submit", axum::routing::post(move |axum::Form(fields): axum::Form<HashMap<String, String>>| {
            let submitted = submitted.clone();
            async move {
                submitted.send(fields).unwrap();
                Html("<!doctype html><title>Submitted</title>Submitted")
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Stop(server);
    let mut config = resolved_config(root.path());
    config.loaded.config.browser.executable = Some(std::path::PathBuf::from(executable));
    config.loaded.config.browser.gateway = openside_config::BrowserGatewaySetting::Auto;
    std::fs::create_dir_all(&config.loaded.paths.runtime_dir).unwrap();
    std::fs::set_permissions(
        &config.loaded.paths.runtime_dir,
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let vault_path = root.path().join("native-vault.db");
    let mut vault = Vault::create(
        &vault_path,
        root.path().join("native-setup-audit.jsonl"),
        &Secret::new("native-test-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "native".into(),
                vec![AllowedOrigin::parse(&origin).unwrap()],
                Secret::new(USERNAME.into()),
                Secret::new(PASSWORD.into()),
                None,
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let pass = root.path().join("native-passphrase");
    std::fs::write(&pass, "native-test-passphrase\n").unwrap();
    config.loaded.config.vault.path = Some(vault_path);
    config.loaded.config.vault.passphrase_cmd =
        Some(vec!["cat".into(), pass.to_string_lossy().into_owned()]);
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let session = create_session(&daemon, "native-vault-session", PolicyMode::Full, None).await;
    let tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":origin}),
            "open-native",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let browser = daemon
        .shared
        .registry
        .browser
        .existing()
        .await
        .map_err(|error| error.0)
        .unwrap();
    let _kill = Killer(browser.process_id());
    assert!(browser.gateway().is_some());
    let fields = refs(&daemon, &session, &tab).await;
    browser.record_protocol(true);
    let outcome = run(
        &daemon,
        &session,
        "vault_login",
        json!({
            "tab_id":tab,"site":"native","username_ref":fields["Username"],
            "password_ref":fields["Password"],"submit_ref":fields["Sign in"],
        }),
        "native-login",
    )
    .await;
    let outcome_text = format!("{outcome:?}");
    assert_clean("native tool outcome", &outcome_text);
    assert_eq!(model(outcome)["status"], "filled");
    let received = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.get("username").map(String::as_str), Some(USERNAME));
    assert_eq!(received.get("password").map(String::as_str), Some(PASSWORD));
    let trace = browser.take_protocol_log();
    assert_clean("agent CDP trace", &serde_json::to_string(&trace).unwrap());
    assert!(!trace
        .iter()
        .any(|(method, _)| method == "Runtime.enable" || method == "Network.enable"));
    assert!(!trace.iter().any(|(method, _)| method == "Input.insertText"));
    let refused_tab = model(
        run(
            &daemon,
            &session,
            "browser_open_tab",
            json!({"url":format!("{origin}/readonly")}),
            "open-native-refusal",
        )
        .await,
    )["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let fields = refs(&daemon, &session, &refused_tab).await;
    let _ = browser.take_protocol_log();
    let failed = run(
        &daemon,
        &session,
        "vault_login",
        json!({
            "tab_id":refused_tab,"site":"native","username_ref":fields["Username"],
            "password_ref":fields["Password"],
        }),
        "native-refusal",
    )
    .await;
    assert_clean("native refusal outcome", &format!("{failed:?}"));
    assert_eq!(model(failed), json!({"status":"fill_failed"}));
    let rejected_trace = browser.take_protocol_log();
    assert_clean(
        "native refusal CDP trace",
        &serde_json::to_string(&rejected_trace).unwrap(),
    );
    assert!(!rejected_trace
        .iter()
        .any(|(method, _)| method == "Input.insertText"
            || method == "Input.dispatchKeyEvent"
            || method == "Runtime.enable"
            || method == "Network.enable"));
    assert_eq!(
        browser
            .evaluate(
                &refused_tab,
                "document.querySelector('input[type=password]').value"
            )
            .await
            .unwrap(),
        ""
    );
    assert_clean(
        "native durable events",
        &serde_json::to_string(&daemon.shared.db.events(&session, 0).await.unwrap()).unwrap(),
    );
    assert_clean(
        "native audit file",
        &std::fs::read_to_string(daemon.shared.config.data_dir.join("vault-audit.jsonl")).unwrap(),
    );
    eprintln!("patched vault: both canaries submitted natively; readonly native refusal had no CDP fallback; trace, outcome, events and audit clean");
    browser.terminate().await.unwrap();
    daemon.shutdown().await;
}

async fn call_as(
    router: &Router,
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
) -> (StatusCode, Value, String) {
    let response = router
        .clone()
        .oneshot(request(method, path, body, token))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        text,
    )
}

/// A paired phone with the admin scope sets up a passphrase vault, adds a login and an
/// authenticator, locks, unlocks and deletes over TCP; values only ever travel inward.
#[tokio::test]
async fn a_paired_admin_device_manages_the_vault_lifecycle_and_items_without_values_coming_back() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    // Pin the passphrase backend: `auto` would use this machine's real keyring.
    config.loaded.config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let tcp = api::router(daemon.shared.clone(), false);
    let (phone_id, phone) = daemon
        .shared
        .db
        .pair_named("phone".into(), vec![CapabilityScope::Admin])
        .await
        .unwrap();
    let (_, reader) = daemon
        .shared
        .db
        .pair_named("reader".into(), vec![CapabilityScope::Read])
        .await
        .unwrap();
    let phone = Some(phone.as_str());
    let passphrase = "correct horse battery";
    let mut seen = String::new();
    let mut send = async |method: &str, path: &str, body: Value| {
        let (status, json, text) = call_as(&tcp, method, path, body, phone).await;
        seen.push_str(&text);
        (status, json)
    };

    let (status, body) = send("GET", "/v1/vault", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"state":"uninitialized","backend":"passphrase","reason":null,"default_policy":"while_unlocked"})
    );
    let (status, body, _) =
        call_as(&tcp, "GET", "/v1/vault", Value::Null, Some(reader.as_str())).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );
    let (status, body, _) = call_as(
        &tcp,
        "POST",
        "/v1/vault/items",
        json!({"id":"bank","origins":["https://bank.example"],"username":USERNAME,"password":PASSWORD}),
        Some(reader.as_str()),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );

    // A passphrase vault is never created implicitly.
    let login = json!({"id":"bank","origins":["https://bank.example"],"username":USERNAME,
        "password":PASSWORD,"totp":null,"policy":"never"});
    let (status, body) = send("POST", "/v1/vault/items", login.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("vault_unavailable"))
    );
    assert!(!daemon.shared.config.data_dir.join("vault.db").exists());
    for (request, code) in [
        (json!({}), "passphrase_required"),
        (json!({"passphrase":"short"}), "weak_passphrase"),
        (
            json!({"passphrase":passphrase,"echo":PASSWORD}),
            "invalid_value",
        ),
        (json!({"passphrase":PASSWORD.len()}), "invalid_value"),
    ] {
        let (status, body) = send("POST", "/v1/vault/setup", request).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some(code))
        );
    }
    let (status, body) = send("POST", "/v1/vault/setup", json!({"passphrase":passphrase})).await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("unlocked"))
    );
    let (status, body) = send("POST", "/v1/vault/setup", json!({"passphrase":passphrase})).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::CONFLICT, Some("vault_already_set_up"))
    );

    let (status, body) = send("POST", "/v1/vault/items", login.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body,
        json!({"id":"bank","origins":["https://bank.example:443"],"policy":"never",
            "effective_policy":"never","has_totp":false,"passkey_rp_ids":[]})
    );
    let authenticator = json!({"id":"otp-github-com","origins":["https://github.com"],
        "totp":format!("otpauth://totp/GitHub:me?secret={SEED}&issuer=GitHub")});
    let (status, body) = send("POST", "/v1/vault/items", authenticator.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        (body["has_totp"].as_bool(), body["policy"].clone()),
        (Some(true), Value::Null)
    );
    for (request, status_code, code) in [
        (login.clone(), StatusCode::CONFLICT, "item_exists"),
        (
            json!({"id":"x","origins":["not an origin"],"password":PASSWORD}),
            StatusCode::BAD_REQUEST,
            "invalid_origin",
        ),
        (
            json!({"id":"x","origins":["https://x.example"],"totp":"not base32 !"}),
            StatusCode::BAD_REQUEST,
            "invalid_totp",
        ),
        (
            json!({"id":"x","origins":["https://x.example"],"username":USERNAME}),
            StatusCode::BAD_REQUEST,
            "empty_item",
        ),
        (
            json!({"id":"Bad Id","origins":["https://x.example"],"password":PASSWORD}),
            StatusCode::BAD_REQUEST,
            "invalid_item",
        ),
        (
            json!({"id":"x","origins":["https://x.example"],"password":PASSWORD,"note":SEED}),
            StatusCode::BAD_REQUEST,
            "invalid_value",
        ),
    ] {
        let (status, body) = send("POST", "/v1/vault/items", request).await;
        assert_eq!((status, body["code"].as_str()), (status_code, Some(code)));
    }

    let (status, body) = send("POST", "/v1/vault/lock", Value::Null).await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("locked"))
    );
    let (_, body) = send("GET", "/v1/vault/items", Value::Null).await;
    assert_eq!(body["available"], json!(false));
    let (status, body) = send(
        "POST",
        "/v1/vault/unlock",
        json!({"passphrase":"wrong passphrase"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("wrong_passphrase"))
    );
    let (status, body) = send("POST", "/v1/vault/unlock", json!({"passphrase":passphrase})).await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("unlocked"))
    );
    let (_, body) = send("GET", "/v1/vault/items", Value::Null).await;
    let ids: Vec<_> = body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].clone())
        .collect();
    assert_eq!(ids, [json!("bank"), json!("otp-github-com")]);

    let (status, _) = send("DELETE", "/v1/vault/items/otp-github-com", Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = send("DELETE", "/v1/vault/items/otp-github-com", Value::Null).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("unknown_item"))
    );
    let (_, body) = send("GET", "/v1/vault/audit", Value::Null).await;
    let actions: Vec<_> = body["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| {
            format!("{}:{}:{}", record["what"], record["item"], record["result"]).replace('"', "")
        })
        .collect();
    assert!(
        actions.contains(&"item_add:bank:added".to_owned()),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"item_add:bank:denied".to_owned()),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"item_delete:otp-github-com:allowed".to_owned()),
        "{actions:?}"
    );
    assert!(
        actions.contains(&"item_delete:unknown-site:deleted".to_owned()),
        "{actions:?}"
    );
    // Lifecycle steps are audited, value-free, and attributed to the calling device; so are
    // item changes (not a generic `api` label).
    let device = format!("device:{phone_id}");
    let lifecycle: Vec<_> = body["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["what"] != "get")
        .map(|record| {
            assert_eq!(record["who"], json!(device), "{record}");
            format!(
                "{}:{}:{}:{}",
                record["what"], record["item"], record["result"], record["detail"]
            )
            .replace('"', "")
        })
        .filter(|line| {
            ["setup:", "unlock:", "lock:"]
                .iter()
                .any(|step| line.starts_with(step))
        })
        .collect();
    assert_eq!(
        lifecycle,
        [
            "setup:*:failed:passphrase_required",
            "setup:*:completed:null",
            "setup:*:failed:vault_already_set_up",
            "lock:*:locked:null",
            "unlock:*:denied:wrong_passphrase",
            "unlock:*:unlocked:null",
        ]
    );

    assert_clean("vault API responses", &seen);
    assert!(
        !seen.contains(passphrase),
        "a response echoed the passphrase"
    );
    let data = &daemon.shared.config.data_dir;
    for file in ["vault.db", "audit.jsonl", "vault-audit.jsonl"] {
        let bytes = std::fs::read(data.join(file)).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        assert_clean(file, &text);
        assert!(!text.contains(passphrase), "{file} holds the passphrase");
    }
}

async fn unlock_as(
    router: &Router,
    token: Option<&str>,
    passphrase: &str,
) -> (StatusCode, Option<String>, Value) {
    let response = router
        .clone()
        .oneshot(request(
            "POST",
            "/v1/vault/unlock",
            json!({"passphrase":passphrase}),
            token,
        ))
        .await
        .unwrap();
    let status = response.status();
    let retry = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        retry,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Wrong passphrases are throttled before any key derivation: per caller after five, with a
/// doubling wait that a success resets; across paired devices after twenty in fifteen
/// minutes; one queued attempt per caller. The local socket is never held back by devices,
/// so the person at the computer can always unlock.
#[tokio::test]
async fn wrong_passphrases_are_throttled_per_device_and_across_devices_but_never_lock_out_local() {
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let base = std::time::Instant::now();
    let offset = Arc::new(parking_lot::Mutex::new(Duration::ZERO));
    let clock = offset.clone();
    daemon
        .shared
        .remote
        .set_clocks(move || base + *clock.lock(), chrono::Utc::now);
    let advance = |secs: u64| *offset.lock() += Duration::from_secs(secs);
    let tcp = api::router(daemon.shared.clone(), false);
    let local = api::router(daemon.shared.clone(), true);
    let passphrase = "correct horse battery";
    let (status, _, _) = call_as(
        &local,
        "POST",
        "/v1/vault/setup",
        json!({"passphrase":passphrase}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let lock = async || {
        let (status, _, _) = call_as(&local, "POST", "/v1/vault/lock", Value::Null, None).await;
        assert_eq!(status, StatusCode::OK);
    };
    lock().await;
    let mut devices = Vec::new();
    for name in ["phone", "tablet", "laptop", "watch", "spare"] {
        let (_, token) = daemon
            .shared
            .db
            .pair_named(name.into(), vec![CapabilityScope::Admin])
            .await
            .unwrap();
        devices.push(token);
    }
    let phone = Some(devices[0].as_str());
    let wrong = "wrong passphrase";

    for _ in 0..5 {
        let (status, retry, body) = unlock_as(&tcp, phone, wrong).await;
        assert_eq!(
            (status, retry, body["code"].as_str()),
            (StatusCode::BAD_REQUEST, None, Some("wrong_passphrase"))
        );
    }
    let (status, retry, body) = unlock_as(&tcp, phone, passphrase).await;
    assert_eq!(
        (status, retry.as_deref(), body["code"].as_str()),
        (
            StatusCode::TOO_MANY_REQUESTS,
            Some("30"),
            Some("too_many_attempts")
        )
    );
    // Another caller is unaffected.
    let (status, _, _) = unlock_as(&local, None, wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    advance(30);
    let (status, _, _) = unlock_as(&tcp, phone, wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, retry, _) = unlock_as(&tcp, phone, wrong).await;
    assert_eq!(
        (status, retry.as_deref()),
        (StatusCode::TOO_MANY_REQUESTS, Some("60"))
    );
    advance(60);
    let (status, _, body) = unlock_as(&tcp, phone, passphrase).await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("unlocked"))
    );
    lock().await;
    // The success reset the phone's count.
    let (status, _, _) = unlock_as(&tcp, phone, wrong).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // One attempt per caller at a time, and few devices at once: queued attempts wait on the
    // vault, the rest are refused before deriving anything.
    let held = daemon.shared.registry.vault.vault.lock().await;
    let queued: Vec<_> = devices[1..3]
        .iter()
        .map(|token| {
            let (tcp, token) = (tcp.clone(), token.clone());
            tokio::spawn(async move { unlock_as(&tcp, Some(token.as_str()), wrong).await })
        })
        .collect();
    until(|| daemon.shared.registry.vault.throttle_in_flight() == 2).await;
    let (status, _, _) = unlock_as(&tcp, Some(devices[1].as_str()), wrong).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, _, _) = unlock_as(&tcp, Some(devices[3].as_str()), wrong).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    drop(held);
    for attempt in queued {
        assert_eq!(attempt.await.unwrap().0, StatusCode::BAD_REQUEST);
    }

    // Paired devices share twenty failures per fifteen minutes; so far the phone failed 7
    // times and the tablet and laptop once each.
    let mut failures = 9;
    'fill: for token in &devices[..4] {
        for _ in 0..4 {
            if failures == 20 {
                break 'fill;
            }
            let (status, _, _) = unlock_as(&tcp, Some(token.as_str()), wrong).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            failures += 1;
        }
    }
    assert_eq!(failures, 20);
    let (status, retry, _) = unlock_as(&tcp, Some(devices[4].as_str()), passphrase).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(retry.is_some());
    let (status, _, body) = unlock_as(&local, None, passphrase).await;
    assert_eq!(
        (status, body["state"].as_str()),
        (StatusCode::OK, Some("unlocked"))
    );

    // Throttled attempts never reach the audit log; every attempt that ran did.
    let (_, body, _) = call_as(&local, "GET", "/v1/vault/audit", Value::Null, None).await;
    let unlocks = body["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["what"] == "unlock")
        .count();
    // Runs: phone 5, local 1, phone 3 (wrong, right, reset check), 2 queued, 11 up to the
    // device limit, the local unlock.
    assert_eq!(unlocks, 23);
    daemon.shutdown().await;
}

/// The API masks a new item's values before saving it, so an item whose completion audit then
/// fails (here, a password that is JSON punctuation of that very record) is still masked.
#[tokio::test]
async fn an_item_saved_without_its_completion_record_is_already_masked() {
    const PUNCTUATION: &str = r#""decision":"added""#;
    let root = tempfile::tempdir().unwrap();
    let mut config = resolved_config(root.path());
    config.loaded.config.vault.unlock = openside_config::VaultUnlockSetting::Passphrase;
    let daemon = Daemon::open_runtime(config, TestRuntime::default())
        .await
        .unwrap();
    let local = api::router(daemon.shared.clone(), true);
    let (status, _, _) = call_as(
        &local,
        "POST",
        "/v1/vault/setup",
        json!({"passphrase":"correct horse battery"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body, _) = call_as(
        &local,
        "POST",
        "/v1/vault/items",
        json!({"id":"odd","origins":["https://odd.example"],"password":PUNCTUATION}),
        None,
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Some("item_added_audit_incomplete")
        )
    );
    let (_, body, _) = call_as(&local, "GET", "/v1/vault/items", Value::Null, None).await;
    assert_eq!(body["items"][0]["id"], "odd", "{body}");
    let masked = daemon
        .shared
        .registry
        .redact(json!(format!("page text {PUNCTUATION} more")))
        .unwrap();
    assert!(!masked.to_string().contains("added"), "{masked}");
    daemon.shutdown().await;
}
