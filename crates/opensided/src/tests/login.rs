//! `request_login` and `request_choice` end to end against a real headless Chromium and a
//! local fixture shaped like Naver's sign-in page: the ticket mirrors the page's own fields and
//! sign-in options, an answer fills and submits the page exactly once and is classified into
//! the next step, saved logins sign in without asking, and the typed values never reach the
//! agent, events, files or the vault audit. Nothing ever brings the tab to the front.

use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;

use axum::{
    extract::{Form, Query},
    response::{Html, IntoResponse, Redirect},
    routing::{get, post},
    Router,
};
use base64::Engine;
use openside_vault::{AccessPolicy, AllowedOrigin, Entry, Secret};

use super::browser::{model, open_tab, reference, resolve_value, tool};
use super::otp::{agent_saw, bounded, live, otp_config, resolve_as, ticket, title, vault_audit};
use super::*;

const USER: &str = "jane@mail.test";
const PASSWORD: &str = "correct-horse-7781";
const OTHER_USER: &str = "bob@mail.test";
const OTHER_PASSWORD: &str = "bob-pass-5521";
/// A 1×1 PNG served as the site's icon.
const ICON: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

/// Naver's sign-in page in miniature: an "ID or phone number" field, a password, a styled
/// keep-signed-in box hidden behind its label, login-mode tabs and provider buttons. A failed
/// attempt comes back with an alert and the identifier echoed into the field, as Naver does.
fn naver_page(error: Option<&str>, id: &str, captcha: bool) -> String {
    let alert = error
        .map(|text| format!("<div class='error_message' role='alert'>{text}</div>"))
        .unwrap_or_default();
    let captcha = if captcha {
        "<div class='g-recaptcha' style='width:300px;height:80px;background:#eee'></div>"
    } else {
        ""
    };
    format!(
        "<!doctype html><html><head><title>NAVER</title><meta property='og:site_name' content='NAVER'>\
         <link rel='icon' href='/favicon.png'></head><body><main>\
         <ul role='tablist'><li><a href='#id' role='tab' aria-selected='true'>ID/전화번호</a></li>\
         <li><a href='#otn' role='tab' aria-selected='false'>일회용 번호</a></li>\
         <li><a href='#qr' role='tab' aria-selected='false'>QR코드</a></li></ul>\
         <form method='post' action='/login'>\
         <input id='id' name='id' type='text' aria-label='아이디 또는 전화번호' placeholder='아이디 또는 전화번호' value='{id}'>\
         <input id='pw' name='pw' type='password' aria-label='비밀번호' placeholder='비밀번호'>\
         <input type='checkbox' id='keep' name='keep' value='on' style='position:absolute;opacity:0;width:1px;height:1px'>\
         <label for='keep'>로그인 상태 유지</label>{alert}{captcha}\
         <button type='submit' class='btn_login'>로그인</button></form>\
         <div class='sns'><button type='button' class='btn_google' onclick=\"location.href='/google'\">Google로 로그인</button>\
         <button type='button' class='btn_apple' onclick=\"location.href='/apple'\">Apple로 로그인</button></div>\
         </main></body></html>"
    )
}

const HOME: &str = "<!doctype html><title>Signed in</title><h1>Welcome back</h1>";
const OTP: &str = "<!doctype html><title>Verify</title><form><label>인증번호<input name='code' autocomplete='one-time-code' maxlength='6'></label><button>확인</button></form>";
/// A verification-method choice as radio buttons; picking one moves on.
const CHOOSE: &str = "<!doctype html><title>Verify</title><main><h2>Choose how to verify</h2>\
     <form><label><input type='radio' name='method' value='sms' onchange=\"location.href='/chosen?m=sms'\">Text message<br>To ***-1234</label>\
     <label><input type='radio' name='method' value='email' onchange=\"location.href='/chosen?m=email'\">Email<br>To j***@mail.test</label>\
     </form></main>";
/// A trust-this-browser prompt whose buttons the agent names by ref.
const TRUST: &str = "<!doctype html><title>Trust</title><main><h2>Trust this browser?</h2>\
     <button onclick=\"location.href='/chosen?m=trust'\">Trust</button>\
     <button onclick=\"location.href='/chosen?m=later'\">Not now</button></main>";
const GOOGLE: &str = "<!doctype html><title>Google</title><h1>Continue with Google</h1>";

#[derive(Clone, Default)]
struct Received(Arc<Mutex<Vec<HashMap<String, String>>>>);

impl Received {
    fn take(&self) -> Vec<HashMap<String, String>> {
        std::mem::take(&mut *self.0.lock())
    }
}

async fn site() -> (String, Received, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let received = Received::default();
    let posts = received.clone();
    let router = Router::new()
        .route(
            "/naver",
            get(|| async { Html(naver_page(None, "", false)) }),
        )
        .route(
            "/login",
            post(move |Form(form): Form<HashMap<String, String>>| {
                let posts = posts.clone();
                async move {
                    posts.0.lock().push(form.clone());
                    let id = form.get("id").cloned().unwrap_or_default();
                    let pw = form.get("pw").cloned().unwrap_or_default();
                    match (id.as_str(), pw.as_str()) {
                        (USER, PASSWORD) | (OTHER_USER, OTHER_PASSWORD) => {
                            Redirect::to("/home").into_response()
                        }
                        ("otp-user", _) => Redirect::to("/otp").into_response(),
                        ("choice-user", _) => Redirect::to("/choose").into_response(),
                        ("captcha-user", _) => Html(naver_page(None, "", true)).into_response(),
                        _ => Html(naver_page(
                            Some("아이디 또는 비밀번호를 잘못 입력했습니다."),
                            &id,
                            false,
                        ))
                        .into_response(),
                    }
                }
            }),
        )
        .route("/home", get(|| async { Html(HOME) }))
        .route("/otp", get(|| async { Html(OTP) }))
        .route("/choose", get(|| async { Html(CHOOSE) }))
        .route("/trust", get(|| async { Html(TRUST) }))
        .route("/google", get(|| async { Html(GOOGLE) }))
        .route(
            "/chosen",
            get(|Query(query): Query<HashMap<String, String>>| async move {
                Html(format!(
                    "<!doctype html><title>Chosen {}</title><h1>Done</h1>",
                    query.get("m").cloned().unwrap_or_default()
                ))
            }),
        )
        .route(
            "/favicon.png",
            get(|| async {
                (
                    [("content-type", "image/png")],
                    base64::engine::general_purpose::STANDARD
                        .decode(ICON)
                        .unwrap(),
                )
            }),
        );
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (origin, received, task)
}

fn saved(origin: &str, site: &str, user: &str, password: &str) -> Entry {
    Entry::new(
        site.into(),
        vec![AllowedOrigin::parse(origin).unwrap()],
        Secret::new(user.into()),
        Secret::new(password.into()),
        None,
        AccessPolicy::Always,
    )
    .unwrap()
}

async fn daemon_with(root: &Path, entries: Vec<Entry>) -> Option<(Daemon, TestRuntime)> {
    let config = otp_config(root, entries)?;
    let runtime = TestRuntime::default();
    let daemon = Daemon::open_runtime(config, runtime.clone()).await.unwrap();
    Some((daemon, runtime))
}

async fn deferred(outcome: ToolOutcome) -> TicketId {
    let ToolOutcome::Deferred {
        ticket,
        kind: DeferKind::External,
    } = outcome
    else {
        panic!("expected a ticket, got {outcome:?}");
    };
    ticket
}

async fn request_login(daemon: &Daemon, session: &SessionId, tab: &str, id: &str) -> ToolOutcome {
    bounded(daemon, session, "request_login", json!({"tab_id":tab}), id).await
}

async fn wait_title(daemon: &Daemon, tab: &str, want: &str) {
    for _ in 0..100 {
        if title(daemon, tab).await == want {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the page never showed {want}");
}

/// No secret anywhere a person or the agent could read it later: events, what the agent was
/// handed, any file under the data directory, and the vault audit.
async fn assert_nowhere(daemon: &Daemon, runtime: &TestRuntime, session: &SessionId, canary: &str) {
    let events =
        serde_json::to_string(&daemon.shared.db.events(session, 0).await.unwrap()).unwrap();
    assert!(!events.contains(canary), "an event carries {canary}");
    assert!(
        !agent_saw(runtime).contains(canary),
        "the agent saw {canary}"
    );
    assert!(!format!("{:?}", runtime.0.lock().opened).contains(canary));
    assert!(!vault_audit(daemon).await.to_string().contains(canary));
    let mut stack = vec![daemon.shared.config.data_dir.clone()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                assert!(
                    !bytes
                        .windows(canary.len())
                        .any(|window| window == canary.as_bytes()),
                    "{} holds {canary}",
                    path.display()
                );
            }
        }
    }
}

fn focus_free(log: &[(String, Value)]) {
    for (method, _) in log {
        assert!(
            !matches!(
                method.as_str(),
                "Page.bringToFront" | "Target.activateTarget"
            ),
            "{method} would steal the person's focus"
        );
    }
}

/// (a) the card mirrors the page; (h) saving needs both scopes; (i) no focus is taken.
#[tokio::test]
async fn a_login_card_mirrors_the_page_and_fills_it_once_without_any_value_escaping() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = daemon_with(root.path(), Vec::new()).await else {
        return;
    };
    let (origin, received, fixture) = site().await;
    let session = create_session(&daemon, "login-card", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/naver"), "login-open").await;
    let browser = live(&daemon).await;
    browser.record_protocol(true);

    let id = deferred(request_login(&daemon, &session, &tab, "login-ask").await).await;
    let opened = ticket(&daemon, &id).await;
    assert_eq!(opened["kind"], "login");
    assert_eq!(opened["target"]["origin"], json!(origin));
    let login = &opened["login"];
    assert_eq!(login["origin"], json!(origin));
    assert_eq!(login["site_name"], "NAVER");
    assert_eq!(
        login["icon"],
        json!(format!("data:image/png;base64,{ICON}"))
    );
    assert_eq!(login["submit_label"], "로그인");
    assert_eq!(login["can_save"], true);
    assert!(login.get("accounts").is_none());
    assert_eq!(
        login["fields"],
        json!([
            {"id":"f0","kind":"identifier","label":"아이디 또는 전화번호","placeholder":"아이디 또는 전화번호",
             "autocomplete":"username","masked":false,"required":true},
            {"id":"f1","kind":"password","label":"비밀번호","placeholder":"비밀번호",
             "autocomplete":"current-password","masked":true,"required":true},
            {"id":"f2","kind":"checkbox","label":"로그인 상태 유지","masked":false,"required":false},
        ])
    );
    assert_eq!(
        login["alternatives"],
        json!([
            {"id":"a0","label":"일회용 번호","kind":"other","provider":null},
            {"id":"a1","label":"QR코드","kind":"qr","provider":null},
            {"id":"a2","label":"Google로 로그인","kind":"provider","provider":"google"},
            {"id":"a3","label":"Apple로 로그인","kind":"provider","provider":"apple"},
        ])
    );

    let answer = json!({"values":{"f0":USER,"f1":PASSWORD,"f2":true},"save":true});
    // (h) Keeping the login is an approval: HumanStep alone is refused before anything is
    // touched, and Approve alone cannot answer a login at all.
    for scopes in [
        vec![CapabilityScope::HumanStep],
        vec![CapabilityScope::Approve],
    ] {
        let (_, token) = daemon.shared.db.pair(scopes.clone()).await.unwrap();
        let (status, body) = resolve_as(&daemon, &token, &id, answer.clone()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{scopes:?}");
        assert_eq!(body["code"], "forbidden");
        assert_eq!(ticket(&daemon, &id).await["status"], "pending");
    }
    assert!(
        received.take().is_empty(),
        "a refused answer reached the page"
    );
    // Malformed answers are refused the same way.
    for bad in [
        json!({"values":{"f0":USER}}),
        json!({"values":{"f0":USER,"f1":PASSWORD,"f9":"x"}}),
        json!({"values":{"f0":USER,"f1":PASSWORD,"f2":"yes"}}),
        json!({"alternative":"a9"}),
        json!("plain"),
    ] {
        let response = resolve_value(&daemon, &id, bad).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    let (device, both) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep, CapabilityScope::Approve])
        .await
        .unwrap();
    let (status, resolved) = resolve_as(&daemon, &both, &id, answer).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(
        resolved["resolution"],
        json!({"status":"submitted","next":"done","saved":true})
    );
    wait_title(&daemon, &tab, "Signed in").await;
    let posts = received.take();
    assert_eq!(posts.len(), 1, "the page was submitted once");
    assert_eq!(posts[0]["id"], USER);
    assert_eq!(posts[0]["pw"], PASSWORD);
    assert_eq!(posts[0].get("keep").map(String::as_str), Some("on"));

    // The login is kept for exactly this origin, saved by the answering device.
    {
        let guard = daemon.shared.registry.vault.vault.lock().await;
        let accounts = guard.as_ref().unwrap().logins_for(&origin).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].username_hint, "j***@mail.test");
    }
    let audit = vault_audit(&daemon).await;
    let save = audit["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["what"] == "login_save" && record["result"] == "saved")
        .cloned()
        .expect("save audited");
    assert_eq!(save["who"], json!(format!("device:{device}")));

    for canary in [USER, PASSWORD] {
        assert_nowhere(&daemon, &runtime, &session, canary).await;
    }
    focus_free(&browser.take_protocol_log());
    browser.record_protocol(false);
    daemon.shutdown().await;
    fixture.abort();
}

/// (b) one saved login signs in without asking; (c) several are offered to pick from.
#[tokio::test]
async fn saved_logins_sign_in_without_asking_or_are_offered_to_pick() {
    let root = tempfile::tempdir().unwrap();
    let (origin, received, fixture) = site().await;
    let Some((daemon, runtime)) = daemon_with(
        root.path(),
        vec![saved(&origin, "naver-jane", USER, PASSWORD)],
    )
    .await
    else {
        fixture.abort();
        return;
    };
    let session = create_session(&daemon, "login-vault", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/naver"), "vault-open").await;
    let browser = live(&daemon).await;
    browser.record_protocol(true);
    let filled = model(request_login(&daemon, &session, &tab, "vault-one").await);
    assert_eq!(
        filled,
        json!({"status":"filled_from_vault","item_id":"naver-jane","next":"done"})
    );
    wait_title(&daemon, &tab, "Signed in").await;
    let posts = received.take();
    assert_eq!(posts.len(), 1);
    assert_eq!(
        (posts[0]["id"].as_str(), posts[0]["pw"].as_str()),
        (USER, PASSWORD)
    );
    let events = daemon.shared.db.events(&session, 0).await.unwrap();
    assert!(!events
        .iter()
        .any(|event| matches!(event.kind, EventKind::TicketOpened { .. })));

    // A second account for the origin: nothing is guessed, the person picks one.
    {
        let mut guard = daemon.shared.registry.vault.vault.lock().await;
        guard
            .as_mut()
            .unwrap()
            .put(saved(&origin, "naver-bob", OTHER_USER, OTHER_PASSWORD))
            .unwrap();
    }
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/naver")}),
            "vault-back",
        )
        .await,
    );
    let id = deferred(request_login(&daemon, &session, &tab, "vault-two").await).await;
    let opened = ticket(&daemon, &id).await;
    let mut accounts = opened["login"]["accounts"].as_array().unwrap().clone();
    accounts.sort_by_key(|account| account["item_id"].as_str().unwrap().to_owned());
    assert_eq!(
        accounts,
        [
            json!({"item_id":"naver-bob","username_hint":"b***@mail.test"}),
            json!({"item_id":"naver-jane","username_hint":"j***@mail.test"}),
        ]
    );
    assert!(received.take().is_empty());
    let response = resolve_value(&daemon, &id, json!({"account":"naver-other"})).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = resolve_value(&daemon, &id, json!({"account":"naver-bob"})).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"status":"filled_from_vault","item_id":"naver-bob","next":"done"})
    );
    let posts = received.take();
    assert_eq!(posts.len(), 1);
    assert_eq!(
        (posts[0]["id"].as_str(), posts[0]["pw"].as_str()),
        (OTHER_USER, OTHER_PASSWORD)
    );
    for canary in [USER, PASSWORD, OTHER_USER, OTHER_PASSWORD] {
        assert_nowhere(&daemon, &runtime, &session, canary).await;
    }
    focus_free(&browser.take_protocol_log());
    browser.record_protocol(false);
    daemon.shutdown().await;
    fixture.abort();
}

/// (d) an alternative is clicked with no values; (e) each next step is recognized; (g) a
/// page replaced after the ticket opened is refused and the ticket stays open.
#[tokio::test]
async fn answers_click_alternatives_classify_the_next_step_and_refuse_a_replaced_page() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, runtime)) = daemon_with(root.path(), Vec::new()).await else {
        return;
    };
    let (origin, received, fixture) = site().await;
    let session = create_session(&daemon, "login-next", PolicyMode::Full, None).await;
    let tab = open_tab(&daemon, &session, &format!("{origin}/naver"), "next-open").await;
    let browser = live(&daemon).await;
    browser.record_protocol(true);
    let naver = |id: &'static str| {
        let daemon = &daemon;
        let session = &session;
        let tab = &tab;
        let origin = &origin;
        async move {
            model(
                tool(
                    daemon,
                    session,
                    "browser_navigate",
                    json!({"tab_id":tab,"url":format!("{origin}/naver")}),
                    &format!("{id}-nav"),
                )
                .await,
            );
            deferred(request_login(daemon, session, tab, id).await).await
        }
    };

    // (g) The tab moved to a new document after the ticket opened.
    let stale = naver("stale").await;
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/naver")}),
            "stale-move",
        )
        .await,
    );
    let response =
        resolve_value(&daemon, &stale, json!({"values":{"f0":USER,"f1":PASSWORD}})).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(response).await["code"], "target_unavailable");
    assert_eq!(ticket(&daemon, &stale).await["status"], "pending");
    assert!(received.take().is_empty());

    // (e) Each kind of next step.
    for (user, password, next) in [
        ("otp-user", "otp-pass-1", "otp"),
        ("choice-user", "choice-pass-2", "choice"),
        ("captcha-user", "captcha-pass-3", "captcha"),
        ("wrong-user", "wrong-pass-4", "error"),
    ] {
        let id = naver(user).await;
        let response = resolve_value(
            &daemon,
            &id,
            json!({"values":{"f0":user,"f1":password,"f2":false}}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{user}");
        let resolution = json_body(response).await["resolution"].clone();
        assert_eq!(resolution["status"], "submitted", "{resolution}");
        assert_eq!(resolution["next"], next, "{resolution}");
        assert!(resolution.get("saved").is_none());
        if next == "error" {
            assert_eq!(
                resolution["error_text"],
                "아이디 또는 비밀번호를 잘못 입력했습니다."
            );
            // The page echoes the identifier; the agent's view of the page never shows it.
            let snapshot = model(
                tool(
                    &daemon,
                    &session,
                    "browser_snapshot",
                    json!({"tab_id":tab}),
                    "error-snapshot",
                )
                .await,
            );
            assert!(!snapshot.to_string().contains(user), "{snapshot}");
        } else {
            assert!(resolution.get("error_text").is_none());
        }
        let posts = received.take();
        assert_eq!(posts.len(), 1, "{user}");
        assert_eq!(posts[0].get("keep"), None, "{user}");
        assert_nowhere(&daemon, &runtime, &session, password).await;
    }

    // (d) An alternative is a click, nothing typed.
    let id = naver("alternative").await;
    let response = resolve_value(&daemon, &id, json!({"alternative":"a2"})).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await["resolution"],
        json!({"status":"alternative","next":"done"})
    );
    wait_title(&daemon, &tab, "Google").await;
    assert!(received.take().is_empty());
    assert!(!daemon
        .shared
        .registry
        .browser
        .fill_intent_pending(&daemon.shared, &id)
        .await
        .unwrap());

    focus_free(&browser.take_protocol_log());
    browser.record_protocol(false);
    daemon.shutdown().await;
    fixture.abort();
}

/// (f) a choice found on the page, and one named by the agent with labels that lie: the
/// person always sees the page's own labels, and the pick is clicked.
#[tokio::test]
async fn choices_show_the_pages_own_labels_and_click_the_pick() {
    let root = tempfile::tempdir().unwrap();
    let Some((daemon, _runtime)) = daemon_with(root.path(), Vec::new()).await else {
        return;
    };
    let (origin, _received, fixture) = site().await;
    let session = create_session(&daemon, "choice", PolicyMode::Full, None).await;
    let tab = open_tab(
        &daemon,
        &session,
        &format!("{origin}/choose"),
        "choice-open",
    )
    .await;
    let browser = live(&daemon).await;
    browser.record_protocol(true);

    let id = deferred(
        bounded(
            &daemon,
            &session,
            "request_choice",
            json!({"tab_id":tab,"question":"ignored when the page has a heading"}),
            "choice-find",
        )
        .await,
    )
    .await;
    let opened = ticket(&daemon, &id).await;
    assert_eq!(opened["kind"], "choice");
    assert_eq!(
        opened["choice"],
        json!({
            "origin":origin,"site_name":"Verify","question":"Choose how to verify",
            "options":[
                {"id":"o0","label":"Text message","description":"To ***-1234"},
                {"id":"o1","label":"Email","description":"To j***@mail.test"},
            ],
            "multiple":false,
        })
    );
    let response = resolve_value(&daemon, &id, json!({"option":"o7"})).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Choosing releases no secret, so it is an approval: HumanStep alone cannot answer it.
    let (_, human) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::HumanStep])
        .await
        .unwrap();
    let (status, _) = resolve_as(&daemon, &human, &id, json!({"option":"o1"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (_, approve) = daemon
        .shared
        .db
        .pair(vec![CapabilityScope::Approve])
        .await
        .unwrap();
    let (status, resolved) = resolve_as(&daemon, &approve, &id, json!({"option":"o1"})).await;
    assert_eq!(status, StatusCode::OK, "{resolved}");
    assert_eq!(
        resolved["resolution"],
        json!({"status":"chosen","option":"o1","next":"done"})
    );
    wait_title(&daemon, &tab, "Chosen email").await;

    // The agent names the options, with labels that do not match the page.
    model(
        tool(
            &daemon,
            &session,
            "browser_navigate",
            json!({"tab_id":tab,"url":format!("{origin}/trust")}),
            "trust-open",
        )
        .await,
    );
    let trust = reference(&daemon, &session, &tab, "Trust", "trust-ref").await;
    let later = reference(&daemon, &session, &tab, "Not now", "later-ref").await;
    let id = deferred(
        bounded(
            &daemon,
            &session,
            "request_choice",
            json!({"tab_id":tab,"options":[
                {"ref":trust,"label":"Delete my account"},
                {"ref":later,"label":"Send all my money"},
            ]}),
            "trust-ask",
        )
        .await,
    )
    .await;
    let opened = ticket(&daemon, &id).await;
    let text = opened.to_string();
    assert!(!text.contains("Delete my account") && !text.contains("Send all my money"));
    assert_eq!(opened["choice"]["question"], "Choose an option");
    assert_eq!(
        opened["choice"]["options"],
        json!([{"id":"o0","label":"Trust"},{"id":"o1","label":"Not now"}])
    );
    let response = resolve_value(&daemon, &id, json!({"option":"o1"})).await;
    assert_eq!(response.status(), StatusCode::OK);
    wait_title(&daemon, &tab, "Chosen later").await;

    // A ref that is not a usable control is refused before any ticket opens.
    let refused = bounded(
        &daemon,
        &session,
        "request_choice",
        json!({"tab_id":tab,"options":[{"ref":"e999"},{"ref":"e998"}]}),
        "choice-bad",
    )
    .await;
    assert!(matches!(refused, ToolOutcome::Denied { .. }), "{refused:?}");

    focus_free(&browser.take_protocol_log());
    browser.record_protocol(false);
    daemon.shutdown().await;
    fixture.abort();
}
