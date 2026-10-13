//! Public API and CLI connection lifecycle with a scripted sidecar and headless local pages.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Form, Json, Router,
};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind, LoginFieldKind, TicketKind};
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");
const PASSWORD: &str = "CONNECTION_LOGIN_PASSWORD_08c722";
const COOKIE: &str = "CONNECTION_COOKIE_CANARY_691e52";
const LOGIN: &str = "<!doctype html><title>Sign in</title><h1>Sign in</h1><form method='post' action='/login'><label>Email<input name='email' type='email' autocomplete='username'></label><label>Password<input name='password' type='password' autocomplete='current-password'></label><button type='submit'>Sign in</button></form>";

struct Site(tokio::task::JoinHandle<()>);
impl Drop for Site {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn authenticated(headers: &HeaderMap) -> bool {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .any(|cookie| cookie.trim() == format!("session={COOKIE}"))
        })
}

async fn login(Form(fields): Form<HashMap<String, String>>) -> Response {
    if fields.get("email").map(String::as_str) != Some("jane@example.test")
        || fields.get("password").map(String::as_str) != Some(PASSWORD)
    {
        return (StatusCode::UNAUTHORIZED, Html(LOGIN)).into_response();
    }
    (
        [(
            header::SET_COOKIE,
            format!("session={COOKIE}; Path=/; Max-Age=3600; HttpOnly; Secure; SameSite=Lax"),
        )],
        Redirect::to("/account"),
    )
        .into_response()
}

async fn account(headers: HeaderMap) -> Response {
    if !authenticated(&headers) {
        return Redirect::to("/login").into_response();
    }
    Html("<!doctype html><title>Account</title><nav><button>Profile: jane@example.test</button><a href='/logout'>Sign out</a></nav><h1>Your account</h1><script>localStorage.setItem('account-marker','present')</script>").into_response()
}

async fn get_json(harness: &Harness, path: &str) -> Value {
    let response = harness.request("GET", path, None).await;
    assert_eq!(response.status, 200, "{path}: {}", response.body);
    response.body
}

async fn tool(harness: &Harness, session: &str, name: &str, args: Value) -> Value {
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/tools/{name}"),
            Some(json!({"args":args})),
        )
        .await;
    assert_eq!(response.status, 200, "{name}: {}", response.body);
    response.body
}

async fn done(harness: &Harness, session: &str, name: &str, args: Value) -> Value {
    let response = tool(harness, session, name, args).await;
    assert_eq!(response["status"], "done", "{name}: {response}");
    response["result"].clone()
}

async fn cli(harness: &Harness, args: &[&str]) -> Vec<Value> {
    let mut argv = vec!["--json", "connections"];
    argv.extend_from_slice(args);
    let output = harness.cli(CLI, &argv, None).await;
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

async fn finish_turn(stream: &mut SseReader, expected: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut replied = false;
    loop {
        let event = stream.next().await;
        if let EventKind::AssistantFinal { text, .. } = &event.kind {
            assert_eq!(text, expected, "{events:?}");
            replied = true;
        }
        let finished = replied && matches!(&event.kind, EventKind::SessionSettled {});
        events.push(event);
        if finished {
            return events;
        }
    }
}

fn result<'a>(events: &'a [Event], call: &str) -> &'a Value {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error: false,
                ..
            } if tool_call_id == call => Some(result),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing successful {call}: {events:?}"))
}

#[tokio::test]
async fn login_teammate_always_fetch_cli_parity_and_revoke_clear_browser_sign_in() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(executable) => executable,
        Err(reason) => {
            eprintln!("SKIP connections requires headless Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route("/login", get(|| async { Html(LOGIN) }).post(login))
        .route("/account", get(account))
        .route("/api/account", get(|headers: HeaderMap| async move {
            if authenticated(&headers) {
                (StatusCode::OK, Json(json!({"signed_in":true,"projects":["Local project"]})))
            } else {
                (StatusCode::UNAUTHORIZED, Json(json!({"signed_in":false})))
            }
        }))
        .route("/inspect", get(|headers: HeaderMap| async move {
            let state = if authenticated(&headers) { "Cookie present" } else { "Cookie absent" };
            Html(format!("<!doctype html><title>Inspection</title><h1>{state}</h1><p id='storage'></p><script>document.getElementById('storage').textContent=localStorage.getItem('account-marker')?'Local data present':'Local data cleared'</script>"))
        }))
        .route("/logout", post(|| async { StatusCode::NO_CONTENT }));
    let _site = Site(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"browser_open_tab","args":{"url":format!("{origin}/login")},"id":"open"}]},
        {"parts":[{"type":"tool_call","name":"request_login","args":{"tab_id":"{{result:open.tab_id}}"},"id":"login"}]},
        {"parts":[{"type":"text","text":"The sign-in is ready."}]}
    ]});
    let Some(mut harness) = Harness::prepare(DAEMON, &script) else {
        return;
    };
    harness.append_config(&format!("[browser]\nexecutable = {}\n", json!(executable)));
    harness.launch().await;
    // The chief front delegates acting tools to Tasks; this legacy browser session signs as chief.
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({
                "title":"Local connection sign-in","mode":"full","workspace":harness.workspace()
            })),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap();
    let mut stream = harness.events(session, 0).await;
    let submitted = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"text":"Sign in to the local account","mode":"now"})),
        )
        .await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    let mut observed = Vec::new();
    let login = loop {
        let event = stream.next().await;
        match &event.kind {
            EventKind::TicketOpened { ticket } => break ticket.clone(),
            EventKind::SessionSettled {} => panic!("settled before a Login ticket: {observed:?}"),
            _ => observed.push(event),
        }
    };
    assert_eq!(login.kind, TicketKind::Login);
    let mut values = serde_json::Map::new();
    for field in &login.login.as_ref().unwrap().fields {
        let value = match field.kind {
            LoginFieldKind::Identifier => "jane@example.test",
            LoginFieldKind::Password => PASSWORD,
            _ => continue,
        };
        values.insert(field.id.clone(), json!(value));
    }
    let answered = harness
        .human_request(
            "POST",
            &format!("/v1/tickets/{}/resolve", login.id),
            Some(json!({"value":{"values":values,"save":false}})),
        )
        .await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    let login_events = finish_turn(&mut stream, "The sign-in is ready.").await;
    let list = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let list = get_json(&harness, "/v1/connections").await;
            if list["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["status"] == "active")
            {
                break list;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("observed sign-in registers an active row");
    let row = &list["connections"][0];
    let id = row["id"].as_str().unwrap();
    assert_eq!(row["site"], "localhost");
    assert_eq!(row["source"], "login");
    assert!(row["grants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|grant| grant["agent_id"] == "chief" && grant["access"] == "allowed"));
    assert!(!row.to_string().contains("jane@example.test"));
    assert_eq!(
        cli(&harness, &["list"]).await[0],
        get_json(&harness, "/v1/connections").await
    );
    assert_eq!(
        cli(&harness, &["list", "--site", "localhost"]).await[0],
        get_json(&harness, "/v1/connections").await
    );
    assert!(
        cli(&harness, &["list", "--site", "example.invalid"]).await[0]["connections"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        cli(&harness, &["show", id]).await[0],
        get_json(&harness, &format!("/v1/connections/{id}")).await
    );
    let checked = cli(&harness, &["check", id]).await;
    assert_eq!(checked[0]["status"], "active");
    assert_eq!(
        checked[0],
        get_json(&harness, &format!("/v1/connections/{id}")).await
    );

    let hired = harness
        .cli(
            CLI,
            &[
                "--json",
                "team",
                "hire",
                "--name",
                "Mina",
                "--role",
                "Checks sign-ins",
            ],
            None,
        )
        .await;
    assert!(
        hired.status.success(),
        "{}",
        String::from_utf8_lossy(&hired.stderr)
    );
    let teammate: Value = serde_json::from_str(
        String::from_utf8_lossy(&hired.stdout)
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let agent = teammate["id"].as_str().unwrap();
    let teammate_session = teammate["home_session_id"].as_str().unwrap();
    let grant = cli(&harness, &["grant", id, agent, "ask"]).await;
    assert_eq!(
        grant[0],
        get_json(&harness, &format!("/v1/connections/{id}")).await
    );
    // Grants are external continuations, so the agent retries open after the person approves.
    harness.replace_script(&json!({"responses":[
        {"parts":[{"type":"tool_call","name":"connection_open","args":{"connection_id":id},"id":"request-connection-grant"}]},
        {"parts":[{"type":"tool_call","name":"connection_open","args":{"connection_id":id},"id":"connection-open"}]},
        {"parts":[{"type":"tool_call","name":"connection_fetch","args":{
            "connection_id":id,"tab_id":"{{result:connection-open.tab_id}}","method":"GET","path":"/api/account"
        },"id":"connection-fetch"}]},
        {"parts":[{"type":"text","text":"Fetched the local account."}]},
        {"parts":[{"type":"tool_call","name":"connection_open","args":{"connection_id":id},"id":"after-revoke"}]},
        {"parts":[{"type":"text","text":"The sign-in was revoked."}]}
    ]}));
    let mut teammate_stream = harness.events(teammate_session, 0).await;
    let submitted = harness.request("POST", &format!("/v1/sessions/{teammate_session}/messages"),
        Some(json!({"message_id":"use-local-connection","text":"Use the local connection and fetch its account","mode":"now"}))).await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    let mut asking = Vec::new();
    let ticket = loop {
        let event = teammate_stream.next().await;
        match &event.kind {
            EventKind::TicketOpened { ticket } => break ticket.clone(),
            EventKind::SessionSettled {} => panic!("teammate settled before asking: {asking:?}"),
            _ => asking.push(event),
        }
    };
    assert_eq!(ticket.kind, TicketKind::Confirm);
    assert!(ticket.summary.contains("Mina"));
    assert!(ticket.summary.contains("localhost"));
    let answered = harness
        .human_request(
            "POST",
            &format!("/v1/tickets/{}/resolve", ticket.id),
            Some(json!({"value":"always"})),
        )
        .await;
    assert_eq!(answered.status, 200, "{}", answered.body);
    let granted = get_json(&harness, &format!("/v1/connections/{id}")).await;
    assert!(granted["grants"]
        .as_array()
        .unwrap()
        .iter()
        .any(|grant| grant["agent_id"] == agent && grant["access"] == "allowed"));
    let used = finish_turn(&mut teammate_stream, "Fetched the local account.").await;
    let opened = result(&used, "connection-open");
    assert!(opened["tab_id"].is_string(), "{used:?}");
    let fetched = result(&used, "connection-fetch");
    assert_eq!(fetched["status"], 200, "{fetched}");
    let body: Value = serde_json::from_str(fetched["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["signed_in"], true);
    assert_eq!(body["projects"], json!(["Local project"]));
    let audit_path = format!("/v1/connections/audit?connection={id}&agent={agent}&action=fetch");
    let audit = cli(
        &harness,
        &[
            "audit",
            "--connection",
            id,
            "--agent",
            agent,
            "--action",
            "fetch",
        ],
    )
    .await;
    assert_eq!(audit[0], get_json(&harness, &audit_path).await);
    assert_eq!(audit[0]["records"][0]["detail"], "GET /api/account");

    let revoked = cli(&harness, &["revoke", id]).await;
    assert_eq!(
        revoked.len(),
        1,
        "204 emits only the existing summary, not a made-up object"
    );
    let missing = harness
        .request("GET", &format!("/v1/connections/{id}"), None)
        .await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.body["code"], "connection_not_found");
    let submitted = harness.request("POST", &format!("/v1/sessions/{teammate_session}/messages"),
        Some(json!({"message_id":"use-after-revoke","text":"Try that connection again","mode":"now"}))).await;
    assert_eq!(submitted.status, 202, "{}", submitted.body);
    let denied = finish_turn(&mut teammate_stream, "The sign-in was revoked.").await;
    assert!(
        denied.iter().any(|event| matches!(&event.kind,
            EventKind::ToolEnded { tool_call_id, result, is_error: true, .. }
                if tool_call_id == "after-revoke" && result["reason"] == "connection_not_found"
        )),
        "{denied:?}"
    );
    let inspect = done(
        &harness,
        teammate_session,
        "browser_open_tab",
        json!({"url":format!("{origin}/inspect")}),
    )
    .await;
    let inspect_tab = inspect["tab_id"].as_str().unwrap();
    let snapshot = done(
        &harness,
        teammate_session,
        "browser_snapshot",
        json!({"tab_id":inspect_tab}),
    )
    .await;
    let snapshot = snapshot.as_str().unwrap();
    assert!(snapshot.contains("Cookie absent"), "{snapshot}");
    assert!(snapshot.contains("Local data cleared"), "{snapshot}");
    let audit = cli(
        &harness,
        &["audit", "--connection", id, "--action", "sign_out"],
    )
    .await;
    assert_eq!(
        audit[0],
        get_json(
            &harness,
            &format!("/v1/connections/audit?connection={id}&action=sign_out")
        )
        .await
    );
    assert_eq!(audit[0]["records"].as_array().unwrap().len(), 1);
    for surface in [
        list.to_string(),
        fetched.to_string(),
        audit[0].to_string(),
        serde_json::to_string(&login_events).unwrap(),
        serde_json::to_string(&used).unwrap(),
        harness.snapshot(session).to_string(),
        harness.snapshot(teammate_session).to_string(),
    ] {
        assert!(!surface.contains(COOKIE));
        assert!(!surface.contains(PASSWORD));
    }
    harness.kill().await;
    for name in [
        "connections.db",
        "connections.db-wal",
        "connections-audit.jsonl",
        "events.db",
        "events.db-wal",
    ] {
        if let Ok(bytes) = std::fs::read(harness.data_dir.join(name)) {
            for canary in [COOKIE, PASSWORD] {
                assert!(
                    !bytes
                        .windows(canary.len())
                        .any(|window| window == canary.as_bytes()),
                    "canary in {name}"
                );
            }
        }
    }
    let logs = std::fs::read_to_string(harness.root().join("daemon.stderr")).unwrap();
    assert!(!logs.contains(COOKIE) && !logs.contains(PASSWORD));
}
