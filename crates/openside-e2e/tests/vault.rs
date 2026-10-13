//! Vault passkeys against a loopback WebAuthn relying party that verifies attestation
//! `none` and ES256 assertions itself (C-2), lookalike-origin and redirect phishing
//! refusal (C-5), and passkey private-key canaries across every recorded surface (C-1).

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Redirect},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use openside_e2e::{Harness, SseReader};
use openside_proto::{Event, EventKind};
use openside_vault::{
    AccessPolicy, AllowedOrigin, AuditContext, Entry, PassphraseLock, Secret, Totp, Vault,
};
use ring::{
    digest::{digest, SHA256},
    rand::{SecureRandom, SystemRandom},
    signature::{UnparsedPublicKey, ECDSA_P256_SHA256_ASN1},
};
use serde_json::{json, Value};

const PASSWORD: &str = "E2E-VAULT-PASSKEY-PASSWORD-CANARY-31c8";
const USER_HANDLE: &[u8] = b"fixture-user-handle";

/// Minimal definite-length CBOR, enough for WebAuthn attestation objects and COSE keys.
#[derive(Debug, Clone, PartialEq)]
enum Cbor {
    Int(i64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
}
fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], String> {
    if input.len() < count {
        return Err("truncated CBOR".into());
    }
    let (head, rest) = input.split_at(count);
    *input = rest;
    Ok(head)
}
fn cbor(input: &mut &[u8]) -> Result<Cbor, String> {
    let head = take(input, 1)?[0];
    let argument = match head & 31 {
        info @ 0..=23 => u64::from(info),
        24 => u64::from(take(input, 1)?[0]),
        25 => u64::from(u16::from_be_bytes(take(input, 2)?.try_into().unwrap())),
        26 => u64::from(u32::from_be_bytes(take(input, 4)?.try_into().unwrap())),
        27 => u64::from_be_bytes(take(input, 8)?.try_into().unwrap()),
        _ => return Err("unsupported CBOR length".into()),
    };
    let length = usize::try_from(argument).map_err(|_| "CBOR length")?;
    Ok(match head >> 5 {
        0 => Cbor::Int(i64::try_from(argument).map_err(|_| "CBOR int")?),
        1 => Cbor::Int(-1 - i64::try_from(argument).map_err(|_| "CBOR int")?),
        2 => Cbor::Bytes(take(input, length)?.to_vec()),
        3 => Cbor::Text(String::from_utf8(take(input, length)?.to_vec()).map_err(|_| "CBOR text")?),
        4 => Cbor::Array((0..length).map(|_| cbor(input)).collect::<Result<_, _>>()?),
        5 => Cbor::Map(
            (0..length)
                .map(|_| Ok((cbor(input)?, cbor(input)?)))
                .collect::<Result<_, String>>()?,
        ),
        _ => return Err("unsupported CBOR type".into()),
    })
}
fn lookup<'a>(map: &'a Cbor, key: &Cbor) -> Result<&'a Cbor, String> {
    let Cbor::Map(entries) = map else {
        return Err("CBOR map expected".into());
    };
    entries
        .iter()
        .find_map(|(k, v)| (k == key).then_some(v))
        .ok_or_else(|| format!("CBOR key {key:?} missing"))
}

struct Stored {
    id: Vec<u8>,
    public_key: Vec<u8>,
    sign_count: u32,
}
struct Rp {
    origin: String,
    challenge: Mutex<Vec<u8>>,
    credential: Mutex<Option<Stored>>,
    registered: AtomicBool,
    signed_in: AtomicBool,
    rejected: Mutex<Vec<String>>,
}
impl Rp {
    fn client_data(&self, encoded: &str, kind: &str) -> Result<Vec<u8>, String> {
        let raw = decode(encoded)?;
        let data: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        let challenge = URL_SAFE_NO_PAD.encode(&*self.challenge.lock().unwrap());
        if data["type"] != kind || data["challenge"] != challenge.as_str() {
            return Err(format!("client data type/challenge mismatch: {data}"));
        }
        if data["origin"] != self.origin.as_str() {
            return Err(format!("client data origin {}", data["origin"]));
        }
        Ok(raw)
    }
    /// rpIdHash, flags and counter shared by both ceremonies.
    fn authenticator_data(&self, data: &[u8], flags: u8) -> Result<u32, String> {
        if data.len() < 37 || data[..32] != *digest(&SHA256, b"localhost").as_ref() {
            return Err("rpIdHash mismatch".into());
        }
        if data[32] & flags != flags {
            return Err(format!("flags {:#x}", data[32]));
        }
        Ok(u32::from_be_bytes(data[33..37].try_into().unwrap()))
    }
    fn register(&self, body: &Value) -> Result<(), String> {
        self.client_data(
            body["client_data"].as_str().unwrap_or(""),
            "webauthn.create",
        )?;
        let attestation =
            cbor(&mut decode(body["attestation"].as_str().unwrap_or(""))?.as_slice())?;
        if *lookup(&attestation, &Cbor::Text("fmt".into()))? != Cbor::Text("none".into())
            || *lookup(&attestation, &Cbor::Text("attStmt".into()))? != Cbor::Map(Vec::new())
        {
            return Err("attestation is not none".into());
        }
        let Cbor::Bytes(auth) = lookup(&attestation, &Cbor::Text("authData".into()))? else {
            return Err("authData".into());
        };
        // UP and AT (attested credential data present).
        let sign_count = self.authenticator_data(auth, 0x41)?;
        let length = usize::from(u16::from_be_bytes(auth[53..55].try_into().unwrap()));
        let id = auth[55..55 + length].to_vec();
        if id != decode(body["id"].as_str().unwrap_or(""))? {
            return Err("credential id mismatch".into());
        }
        let key = cbor(&mut &auth[55 + length..])?;
        let int = |value| Cbor::Int(value);
        if *lookup(&key, &int(1))? != int(2)
            || *lookup(&key, &int(3))? != int(-7)
            || *lookup(&key, &int(-1))? != int(1)
        {
            return Err("COSE key is not ES256 P-256".into());
        }
        let (Cbor::Bytes(x), Cbor::Bytes(y)) = (lookup(&key, &int(-2))?, lookup(&key, &int(-3))?)
        else {
            return Err("COSE coordinates".into());
        };
        let mut public_key = vec![4];
        public_key.extend(x);
        public_key.extend(y);
        *self.credential.lock().unwrap() = Some(Stored {
            id,
            public_key,
            sign_count,
        });
        self.registered.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn login(&self, body: &Value) -> Result<(), String> {
        let client =
            self.client_data(body["client_data"].as_str().unwrap_or(""), "webauthn.get")?;
        let auth = decode(body["authenticator_data"].as_str().unwrap_or(""))?;
        let sign_count = self.authenticator_data(&auth, 0x01)?;
        if decode(body["user_handle"].as_str().unwrap_or(""))? != USER_HANDLE {
            return Err("user handle mismatch".into());
        }
        let mut stored = self.credential.lock().unwrap();
        let stored = stored.as_mut().ok_or("no registered credential")?;
        if decode(body["id"].as_str().unwrap_or(""))? != stored.id {
            return Err("unknown credential".into());
        }
        if sign_count <= stored.sign_count {
            return Err(format!("sign count {sign_count} did not increase"));
        }
        let mut signed = auth.clone();
        signed.extend_from_slice(digest(&SHA256, &client).as_ref());
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, &stored.public_key)
            .verify(&signed, &decode(body["signature"].as_str().unwrap_or(""))?)
            .map_err(|_| "ES256 signature invalid")?;
        stored.sign_count = sign_count;
        self.signed_in.store(true, Ordering::SeqCst);
        Ok(())
    }
}
fn decode(text: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD.decode(text).map_err(|e| e.to_string())
}
async fn options(State(rp): State<Arc<Rp>>) -> Json<Value> {
    let mut challenge = vec![0; 32];
    SystemRandom::new().fill(&mut challenge).unwrap();
    let encoded = URL_SAFE_NO_PAD.encode(&challenge);
    *rp.challenge.lock().unwrap() = challenge;
    Json(json!({"challenge": encoded, "user_id": URL_SAFE_NO_PAD.encode(USER_HANDLE)}))
}
fn verdict(rp: &Rp, result: Result<(), String>) -> StatusCode {
    match result {
        Ok(()) => StatusCode::NO_CONTENT,
        Err(reason) => {
            rp.rejected.lock().unwrap().push(reason);
            StatusCode::BAD_REQUEST
        }
    }
}
async fn register(State(rp): State<Arc<Rp>>, Json(body): Json<Value>) -> StatusCode {
    verdict(&rp, rp.register(&body))
}
async fn assert_login(State(rp): State<Arc<Rp>>, Json(body): Json<Value>) -> StatusCode {
    verdict(&rp, rp.login(&body))
}

const SCRIPT: &str = r"
const b64u = b => btoa(String.fromCharCode(...new Uint8Array(b))).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
const unb64u = s => Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')), c => c.charCodeAt(0));
const done = t => { document.getElementById('status').textContent = t; };
async function post(path, body) { const r = await fetch(path, {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify(body)}); return r.ok; }
async function reg() { try {
  const o = await (await fetch('/options')).json();
  const c = await navigator.credentials.create({publicKey:{challenge:unb64u(o.challenge), rp:{id:'localhost',name:'Fixture RP'},
    user:{id:unb64u(o.user_id),name:'fixture',displayName:'Fixture'}, pubKeyCredParams:[{type:'public-key',alg:-7}],
    attestation:'none', authenticatorSelection:{residentKey:'required',userVerification:'preferred'}}});
  done(await post('/register', {id:c.id, client_data:b64u(c.response.clientDataJSON), attestation:b64u(c.response.attestationObject)}) ? 'registered' : 'rejected');
} catch (e) { done('error ' + e.name); } }
async function signin() { try {
  const o = await (await fetch('/options')).json();
  const c = await navigator.credentials.get({publicKey:{challenge:unb64u(o.challenge), rpId:'localhost', userVerification:'preferred'}});
  done(await post('/assert', {id:c.id, client_data:b64u(c.response.clientDataJSON), authenticator_data:b64u(c.response.authenticatorData),
    signature:b64u(c.response.signature), user_handle:b64u(c.response.userHandle)}) ? 'signed in' : 'rejected');
} catch (e) { done('error ' + e.name); } }
";
fn page(body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><title>Passkey fixture</title>{body}<p id='status'>idle</p><script>{SCRIPT}</script>"
    ))
}
fn register_page() -> Html<String> {
    page("<button aria-label='Create passkey' onclick='reg()'>Create passkey</button>")
}
fn login_page() -> Html<String> {
    page(
        "<label>Username<input aria-label='Username' id='username'></label>\
<label>Password<input aria-label='Password' id='password' type='password'></label>\
<button aria-label='Sign in with passkey' onclick='signin()'>Sign in with passkey</button>",
    )
}

struct Sites {
    rp: Arc<Rp>,
    lookalike: String,
    lookalike_hits: Arc<AtomicUsize>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Sites {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
async fn sites() -> Sites {
    let evil_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    // A lookalike host under a different origin; Chromium resolves *.localhost to loopback.
    let lookalike = format!(
        "http://login.localhost:{}",
        evil_listener.local_addr().unwrap().port()
    );
    let lookalike_hits = Arc::new(AtomicUsize::new(0));
    let hits = lookalike_hits.clone();
    let evil = Router::new()
        .route("/login", get(|| async { login_page() }))
        .route("/options", get(options))
        // Browsers fetch /favicon.ico on their own; only submissions carry credentials.
        .fallback(move |method: axum::http::Method| {
            let hits = hits.clone();
            async move {
                if method != axum::http::Method::GET {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                StatusCode::NOT_FOUND
            }
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rp = Arc::new(Rp {
        origin: format!("http://localhost:{}", listener.local_addr().unwrap().port()),
        challenge: Mutex::new(Vec::new()),
        credential: Mutex::new(None),
        registered: AtomicBool::new(false),
        signed_in: AtomicBool::new(false),
        rejected: Mutex::new(Vec::new()),
    });
    let evil_rp = Arc::new(Rp {
        origin: lookalike.clone(),
        challenge: Mutex::new(Vec::new()),
        credential: Mutex::new(None),
        registered: AtomicBool::new(false),
        signed_in: AtomicBool::new(false),
        rejected: Mutex::new(Vec::new()),
    });
    let target = format!("{lookalike}/login");
    let app = Router::new()
        .route("/register", get(|| async { register_page() }))
        .route("/login", get(|| async { login_page() }))
        .route("/options", get(options))
        .route("/register", post(register))
        .route("/assert", post(assert_login))
        .route(
            "/redirect",
            get(move || {
                let target = target.clone();
                async move { Redirect::temporary(&target) }
            }),
        )
        .with_state(rp.clone());
    let evil = evil.with_state(evil_rp);
    let tasks = vec![
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
        tokio::spawn(async move { axum::serve(evil_listener, evil).await.unwrap() }),
    ];
    Sites {
        rp,
        lookalike,
        lookalike_hits,
        tasks,
    }
}

fn text(text: &str) -> Value {
    json!({"parts":[{"type":"text","text":text}]})
}
fn tool(name: &str, args: Value, id: &str) -> Value {
    json!({"parts":[{"type":"tool_call","name":name,"args":args,"id":id}]})
}
fn ended<'a>(events: &'a [Event], call: &str) -> (&'a Value, bool) {
    events
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error,
                ..
            } if tool_call_id == call => Some((result, *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {call}: {events:?}"))
}
fn ok<'a>(events: &'a [Event], call: &str) -> &'a Value {
    let (result, is_error) = ended(events, call);
    assert!(!is_error, "{call} failed: {result}");
    result
}
fn denied(events: &[Event], call: &str) -> String {
    let (result, is_error) = ended(events, call);
    assert!(
        is_error && result["status"] == "denied",
        "{call} not denied: {result}"
    );
    result["reason"].as_str().unwrap().to_owned()
}
fn field(snapshot: &Value, name: &str) -> String {
    let text = snapshot.as_str().expect("compact accessibility snapshot");
    let label = format!("\"{name}\"");
    let line = text
        .lines()
        .find(|line| line.contains(&label) && (line.contains("textbox") || line.contains("button")))
        .unwrap_or_else(|| panic!("snapshot lacks {name}: {text}"));
    let start = line.find('[').unwrap() + 1;
    let end = line[start..].find(']').unwrap() + start;
    line[start..end].to_owned()
}

/// Appends scripted model turns; a fresh scripted sidecar resumes at the cursor, which
/// lets the script use tab ids and refs returned by earlier real tool calls.
async fn step(
    harness: &mut Harness,
    stream: &mut SseReader,
    responses: &mut Vec<Value>,
    session: &str,
    additions: Vec<Value>,
) -> Vec<Event> {
    step_with(harness, stream, responses, session, additions, None).await
}
/// Like [`step`], with scripted turns for the subagents the added turns delegate to, keyed
/// by the parent's `delegate` tool call id.
async fn step_with(
    harness: &mut Harness,
    stream: &mut SseReader,
    responses: &mut Vec<Value>,
    session: &str,
    additions: Vec<Value>,
    children: Option<&Value>,
) -> Vec<Event> {
    let final_text = additions
        .last()
        .and_then(|turn| turn["parts"][0]["text"].as_str())
        .unwrap()
        .to_owned();
    if !responses.is_empty() {
        let script = [responses.clone(), additions.clone()].concat();
        let mut script = json!({"responses": script});
        if let Some(children) = children {
            script["subagents"] = children.clone();
        }
        harness.replace_script(&script);
        harness.kill_sidecar();
        // A settle left over from the previous turn may still be in flight; the kill's own
        // boundary is the settle that follows its `interrupted` error.
        loop {
            let boundary = stream.settled().await;
            if boundary.iter().any(
                |event| matches!(&event.kind, EventKind::RuntimeError { code, .. } if code == "interrupted"),
            ) {
                break;
            }
        }
    }
    responses.extend(additions);
    let id = format!("message-{}", responses.len());
    let response = harness
        .request(
            "POST",
            &format!("/v1/sessions/{session}/messages"),
            Some(json!({"message_id":id,"text":id,"mode":"now"})),
        )
        .await;
    assert_eq!(response.status, 202, "{}", response.body);
    let events = stream.settled().await;
    assert!(
        events.iter().any(
            |event| matches!(&event.kind, EventKind::AssistantFinal { text } if *text == final_text)
        ),
        "turn missing final {final_text}: {events:?}"
    );
    events
}
async fn eventually(what: &str, condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test]
async fn passkey_register_login_mock_rp_lookalike_and_redirect_denied_without_key_leaks() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: passkey acceptance requires Chromium: {reason}");
            return;
        }
    };
    let sites = sites().await;
    let origin = sites.rp.origin.clone();
    let open = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/register")}),
            "open",
        ),
        text("opened"),
    ];
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":open}),
    ) else {
        return;
    };
    let vault_path = harness.root().join("fixture.vault");
    let passphrase = Secret::new("disposable-passkey-passphrase".into());
    let mut vault = Vault::create(
        &vault_path,
        harness.root().join("setup.audit.jsonl"),
        &passphrase,
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(&origin).unwrap()],
                Secret::new("fixture-user".into()),
                Secret::new(PASSWORD.into()),
                None,
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let unlock = harness.root().join("unlock.sh");
    std::fs::write(
        &unlock,
        "#!/bin/sh\nprintf '%s\\n' 'disposable-passkey-passphrase'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unlock, std::fs::Permissions::from_mode(0o700)).unwrap();
    harness.append_config(&format!(
        "[browser]\nexecutable = {}\n[vault]\npath = {}\npassphrase_cmd = [{}]\n",
        json!(executable),
        json!(vault_path),
        json!(unlock)
    ));
    harness.launch().await;
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let mut stream = harness.events(&session, 0).await;
    let mut responses = Vec::new();
    let mut all = step(&mut harness, &mut stream, &mut responses, &session, open).await;
    let tab = ok(&all, "open")["tab_id"].as_str().unwrap().to_owned();

    // Registration: the authenticator exists only for this call, and the RP checks
    // attestation `none`, the origin, rpIdHash and the COSE ES256 key itself.
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool(
                "browser_snapshot",
                json!({"tab_id":tab}),
                "register-snapshot",
            ),
            text("register page"),
        ],
    )
    .await;
    let create = field(ok(&events, "register-snapshot"), "Create passkey");
    all.extend(events);
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool(
                "vault_passkey",
                json!({"tab_id":tab,"site":"fixture","action":"register","submit_ref":create}),
                "passkey-register",
            ),
            text("registered"),
        ],
    )
    .await;
    assert_eq!(
        *ok(&events, "passkey-register"),
        json!({"status":"registered"})
    );
    all.extend(events);
    eventually("RP registration", || {
        sites.rp.registered.load(Ordering::SeqCst)
    })
    .await;

    // Login: the stored key is loaded into a fresh authenticator, and the RP verifies
    // the ES256 signature over authenticatorData || SHA-256(clientDataJSON).
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool(
                "browser_navigate",
                json!({"tab_id":tab,"url":format!("{origin}/login")}),
                "to-login",
            ),
            tool("browser_snapshot", json!({"tab_id":tab}), "login-snapshot"),
            text("login page"),
        ],
    )
    .await;
    let sign_in = field(ok(&events, "login-snapshot"), "Sign in with passkey");
    all.extend(events);
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool(
                "vault_passkey",
                json!({"tab_id":tab,"site":"fixture","action":"login","submit_ref":sign_in}),
                "passkey-login",
            ),
            tool("browser_snapshot", json!({"tab_id":tab}), "after-login"),
            tool(
                "browser_screenshot",
                json!({"tab_id":tab}),
                "after-login-screenshot",
            ),
            text("signed in"),
        ],
    )
    .await;
    assert_eq!(*ok(&events, "passkey-login"), json!({"status":"signed_in"}));
    all.extend(events);
    eventually("RP assertion", || sites.rp.signed_in.load(Ordering::SeqCst)).await;
    assert!(
        sites.rp.rejected.lock().unwrap().is_empty(),
        "{:?}",
        sites.rp.rejected.lock().unwrap()
    );

    // Lookalike origin and a login link that redirects there: no passkey, no password.
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool(
                "browser_open_tab",
                json!({"url":format!("{}/login", sites.lookalike)}),
                "open-lookalike",
            ),
            tool(
                "browser_open_tab",
                json!({"url":format!("{origin}/redirect")}),
                "open-redirect",
            ),
            text("phish opened"),
        ],
    )
    .await;
    let lookalike_tab = ok(&events, "open-lookalike")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let redirect_tab = ok(&events, "open-redirect")["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    all.extend(events);
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool(
                "browser_snapshot",
                json!({"tab_id":lookalike_tab}),
                "lookalike-snapshot",
            ),
            tool(
                "browser_snapshot",
                json!({"tab_id":redirect_tab}),
                "redirect-snapshot",
            ),
            text("phish fields"),
        ],
    )
    .await;
    let lookalike = ok(&events, "lookalike-snapshot").clone();
    let redirected = ok(&events, "redirect-snapshot").clone();
    all.extend(events);
    let events = step(&mut harness, &mut stream, &mut responses, &session, vec![
        tool("vault_passkey", json!({"tab_id":lookalike_tab,"site":"fixture","action":"login","submit_ref":field(&lookalike, "Sign in with passkey")}), "lookalike-passkey"),
        tool("vault_passkey", json!({"tab_id":redirect_tab,"site":"fixture","action":"register","submit_ref":field(&redirected, "Sign in with passkey")}), "redirect-passkey"),
        tool("vault_login", json!({"tab_id":redirect_tab,"site":"fixture","username_ref":field(&redirected, "Username"),"password_ref":field(&redirected, "Password"),"submit_ref":field(&redirected, "Sign in with passkey")}), "redirect-login"),
        tool("browser_tabs", json!({}), "tabs"),
        text("phish refused"),
    ]).await;
    assert_eq!(
        denied(&events, "lookalike-passkey"),
        "vault_origin_mismatch"
    );
    assert_eq!(denied(&events, "redirect-passkey"), "vault_origin_mismatch");
    assert_eq!(denied(&events, "redirect-login"), "vault_origin_mismatch");
    let tabs = ok(&events, "tabs").to_string();
    assert!(
        tabs.contains(&sites.lookalike),
        "redirect did not land on the lookalike: {tabs}"
    );
    all.extend(events);
    // The lookalike never got a credential POST; its page never left the initial state.
    assert_eq!(sites.lookalike_hits.load(Ordering::SeqCst), 0);
    let audit = harness
        .request("GET", &format!("/v1/vault/audit?session={session}"), None)
        .await;
    let records: Vec<(String, String, String)> = audit.body["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["tool_call_id"].as_str().unwrap().to_owned(),
                r["what"].as_str().unwrap().to_owned(),
                r["result"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    for expected in [
        ("passkey-register", "passkey_register", "completed"),
        ("passkey-login", "passkey_login", "completed"),
        ("lookalike-passkey", "passkey_login", "denied"),
        ("redirect-passkey", "passkey_register", "denied"),
        ("redirect-login", "fill", "denied"),
    ] {
        let expected = (
            expected.0.to_owned(),
            expected.1.to_owned(),
            expected.2.to_owned(),
        );
        assert!(
            records.contains(&expected),
            "{expected:?} missing from {records:?}"
        );
    }
    let items = harness.request("GET", "/v1/vault/items", None).await;
    assert_eq!(
        items.body["items"][0]["passkey_rp_ids"],
        json!(["localhost"])
    );

    let transcript = harness.snapshot(&session).to_string();
    let events_text = serde_json::to_string(&all).unwrap();
    harness.kill().await;

    // C-1: read the stored private key back through the vault's own release path and
    // require it (and the password) to be absent from every recorded surface.
    let mut vault = Vault::open(
        &vault_path,
        harness.root().join("inspect.audit.jsonl"),
        &passphrase,
        PassphraseLock::default(),
    )
    .unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink = captured.clone();
    let ctx = AuditContext {
        who: "test",
        session_id: None,
        tool_call_id: None,
    };
    vault
        .passkey_login(&ctx, "fixture", &origin, move |keys| {
            sink.lock()
                .unwrap()
                .extend(keys.into_iter().map(|key| key.private_key.to_string()));
            Box::pin(async { Err(()) })
        })
        .await
        .unwrap();
    drop(vault);
    let keys = captured.lock().unwrap().clone();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].len() > 40);
    for surface in [&transcript, &events_text] {
        assert!(!surface.contains(&keys[0]) && !surface.contains(PASSWORD));
    }
    harness.assert_no_canaries(&[&keys[0], PASSWORD]);
}

const USERNAME: &str = "e2e-vault-user-canary-77a1";
// Base32 of "e2e-otp-seed-canary!".
const SEED: &str = "MUZGKLLPORYC243FMVSC2Y3BNZQXE6JB";
/// Not the default 6, so a code length that ignores the stored `otpauth://` parameters fails.
const OTP_DIGITS: usize = 7;

/// Reports only the shape of the fields (type, value lengths, whether the code is six
/// digits), never a value, so the mock site cannot become a leak path of its own.
const FILL_PAGE: &str = r#"<!doctype html><title>Login</title>
<label>Username<input aria-label="Username" id="username"></label>
<label>Password<input aria-label="Password" id="password" type="password"></label>
<label>Code<input aria-label="Code" id="code"></label>
<script>
const field = (id) => document.getElementById(id);
setInterval(() => fetch('/state?type=' + field('password').type + '&user=' + field('username').value.length
  + '&pass=' + field('password').value.length + '&code=' + field('code').value.length
  + '&digits=' + /^[0-9]+$/.test(field('code').value)), 100);
</script>"#;

fn code(language_tool: &str, tab: &str, source: &str, call: &str) -> Value {
    tool(language_tool, json!({"tab_id":tab,"code":source}), call)
}

#[tokio::test]
async fn password_and_otp_fill_reach_no_surface_and_every_value_read_is_blocked() {
    let executable = match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: vault fill acceptance requires Chromium: {reason}");
            return;
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let page_state = Arc::new(Mutex::new(HashMap::<String, String>::new()));
    let sink = page_state.clone();
    let app = Router::new()
        .route("/fill", get(|| async { Html(FILL_PAGE) }))
        .route(
            "/state",
            get(
                move |Query(query): Query<HashMap<String, String>>| async move {
                    *sink.lock().unwrap() = query;
                },
            ),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let open = vec![
        tool(
            "browser_open_tab",
            json!({"url":format!("{origin}/fill")}),
            "open",
        ),
        text("opened"),
    ];
    let Some(mut harness) = Harness::prepare(
        env!("CARGO_BIN_EXE_e2e-opensided"),
        &json!({"responses":open}),
    ) else {
        return;
    };
    let vault_path = harness.root().join("fill.vault");
    let mut vault = Vault::create(
        &vault_path,
        harness.root().join("setup.audit.jsonl"),
        &Secret::new("disposable-fill-passphrase".into()),
        PassphraseLock::default(),
    )
    .unwrap();
    vault
        .put(
            Entry::new(
                "fixture".into(),
                vec![AllowedOrigin::parse(&origin).unwrap()],
                Secret::new(USERNAME.into()),
                Secret::new(PASSWORD.into()),
                Some(
                    Totp::parse(Secret::new(format!(
                        "otpauth://totp/Fixture:user?secret={SEED}&digits={OTP_DIGITS}&algorithm=SHA256&period=30"
                    )))
                    .unwrap(),
                ),
                AccessPolicy::Always,
            )
            .unwrap(),
        )
        .unwrap();
    drop(vault);
    let unlock = harness.root().join("unlock.sh");
    std::fs::write(
        &unlock,
        "#!/bin/sh\nprintf '%s\\n' 'disposable-fill-passphrase'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&unlock, std::fs::Permissions::from_mode(0o700)).unwrap();
    harness.append_config(&format!(
        "[browser]\nexecutable = {}\n[vault]\npath = {}\npassphrase_cmd = [{}]\n",
        json!(executable),
        json!(vault_path),
        json!(unlock)
    ));
    harness.launch().await;
    let created = harness
        .request(
            "POST",
            "/v1/sessions",
            Some(json!({"mode":"full","workspace":harness.workspace()})),
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.body);
    let session = created.body["id"].as_str().unwrap().to_owned();
    let mut stream = harness.events(&session, 0).await;
    let mut responses = Vec::new();
    let mut all = step(&mut harness, &mut stream, &mut responses, &session, open).await;
    let tab = ok(&all, "open")["tab_id"].as_str().unwrap().to_owned();

    // Real fills of the password and the one-time code into the live page.
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab}), "before"),
            text("fields"),
        ],
    )
    .await;
    let (username, password, one_time) = {
        let snapshot = ok(&events, "before");
        (
            field(snapshot, "Username"),
            field(snapshot, "Password"),
            field(snapshot, "Code"),
        )
    };
    all.extend(events);
    let events = step(&mut harness, &mut stream, &mut responses, &session, vec![
        tool("vault_login", json!({"tab_id":tab,"site":"fixture","username_ref":username,"password_ref":password}), "fill-login"),
        tool("vault_otp", json!({"tab_id":tab,"site":"fixture","field_ref":one_time}), "fill-otp"),
        text("filled"),
    ]).await;
    assert_eq!(*ok(&events, "fill-login"), json!({"status":"filled"}));
    assert_eq!(*ok(&events, "fill-otp"), json!({"status":"filled"}));
    all.extend(events);
    // The page really holds the values, so the absence checks below are not vacuous.
    let wanted = |state: &HashMap<String, String>| {
        state
            .get("pass")
            .is_some_and(|len| *len == PASSWORD.len().to_string())
            && state
                .get("user")
                .is_some_and(|len| *len == USERNAME.len().to_string())
            && state
                .get("code")
                .is_some_and(|len| *len == OTP_DIGITS.to_string())
            && state.get("digits").is_some_and(|digits| digits == "true")
    };
    if tokio::time::timeout(Duration::from_secs(10), async {
        while !wanted(&page_state.lock().unwrap()) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_err()
    {
        panic!(
            "the page never reported the filled fields; last report: {:?}",
            page_state.lock().unwrap()
        );
    }

    // After the fill: every way a tool could read the page or the files.
    let events = step(
        &mut harness,
        &mut stream,
        &mut responses,
        &session,
        vec![
            tool("browser_snapshot", json!({"tab_id":tab}), "after"),
            text("after fill"),
        ],
    )
    .await;
    let (username, password) = {
        let snapshot = ok(&events, "after");
        assert!(
            snapshot.as_str().unwrap().contains("Password"),
            "{snapshot}"
        );
        (field(snapshot, "Username"), field(snapshot, "Password"))
    };
    all.extend(events);
    let vault_file = vault_path.display().to_string();
    let data_dir = harness.data_dir.display().to_string();
    let events = step(&mut harness, &mut stream, &mut responses, &session, vec![
        tool("browser_screenshot", json!({"tab_id":tab}), "shot"),
        code("browser_repl", &tab, "await page.locator('#password').inputValue()", "repl-value"),
        code("browser_repl", &tab, "const v = await page.locator('#password').inputValue(); v.split('').reverse().join('')", "repl-reverse"),
        text("reads refused"),
    ]).await;
    assert_eq!(
        denied(&events, "shot"),
        "screenshot_unavailable_after_secret_fill"
    );
    for call in ["repl-value", "repl-reverse"] {
        assert_eq!(
            denied(&events, call),
            "repl_unavailable_after_secret_fill",
            "{call}"
        );
    }
    all.extend(events);
    // A second run: the sidecar gives up on a tool after a few refused calls in one run.
    let events = step(&mut harness, &mut stream, &mut responses, &session, vec![
        code("browser_repl", &tab, "await page.content()", "repl-content"),
        code("browser_repl", &tab, "await page.evaluate(() => document.getElementById('username').value)", "repl-eval"),
        tool("browser_act", json!({"tab_id":tab,"target":password,"action":"click"}), "act-click"),
        tool("browser_act", json!({"tab_id":tab,"target":username,"action":"hover"}), "act-hover"),
        tool("bash", json!({"command":format!("cat '{vault_file}'; env")}), "bash-vault"),
        tool("bash", json!({"command":format!("find '{data_dir}' -type f -exec cat {{}} + | head -c 1500000")}), "bash-data"),
        tool("read", json!({"path":if cfg!(target_os = "linux") { "/proc/self/environ" } else { "/dev/null" }}), "read-environ"),
        tool("memory_search", json!({"query":"password"}), "memory"),
        text("probed"),
    ]).await;
    for call in ["repl-content", "repl-eval"] {
        assert_eq!(
            denied(&events, call),
            "repl_unavailable_after_secret_fill",
            "{call}"
        );
    }
    for call in ["act-click", "act-hover", "memory"] {
        ok(&events, call);
    }
    // macOS has no proc environ file; its native device filesystem must also remain unreadable.
    assert!(
        denied(&events, "read-environ").contains("protected"),
        "read-environ must be refused as a protected path"
    );
    // This fixture has no provider credentials; encrypted vault bytes must reveal no plaintext.
    for call in ["bash-vault", "bash-data"] {
        let output = ok(&events, call);
        // Oversized results retain a JSON prefix, with exit_code before escaped stdout.
        let exit_code = output["exit_code"].as_i64().or_else(|| {
            if output["truncated"] != true {
                return None;
            }
            output["preview"]
                .as_str()?
                .strip_prefix("{\"exit_code\":")?
                .split_once(',')?
                .0
                .parse::<i64>()
                .ok()
        });
        assert_eq!(exit_code, Some(0), "{call}: {output}");
    }
    assert!(
        ok(&events, "bash-vault").to_string().contains("PATH="),
        "bash printed no environment"
    );
    all.extend(events);

    // A subagent attempts the same reads on the same filled tab.
    let children = json!({"delegate-call": [
        tool("browser_snapshot", json!({"tab_id":tab}), "child-snapshot"),
        tool("browser_screenshot", json!({"tab_id":tab}), "child-shot"),
        code("browser_repl", &tab, "await page.locator('#password').inputValue()", "child-repl"),
        tool("bash", json!({"command":format!("cat '{vault_file}'; env")}), "child-bash"),
        tool("memory_search", json!({"query":"password"}), "child-memory"),
        text("child done"),
    ]});
    let events = step_with(&mut harness, &mut stream, &mut responses, &session, vec![
        tool("delegate", json!({
            "task": "Read the password field of the open login tab and report its value.",
            "tools": ["browser_snapshot", "browser_screenshot", "browser_repl", "bash", "memory_search"],
        }), "delegate-call"),
        text("parent done"),
    ], Some(&children)).await;
    assert_eq!(
        denied(&events, "child-shot"),
        "screenshot_unavailable_after_secret_fill"
    );
    assert_eq!(
        denied(&events, "child-repl"),
        "repl_unavailable_after_secret_fill"
    );
    for call in ["child-snapshot", "child-bash", "child-memory"] {
        ok(&events, call);
    }
    all.extend(events);

    // Nothing above touched the page: still a password field holding both values.
    let state = page_state.lock().unwrap().clone();
    assert_eq!(state.get("type").map(String::as_str), Some("password"));
    assert_eq!(state.get("pass").cloned(), Some(PASSWORD.len().to_string()));
    let audit = harness
        .request("GET", &format!("/v1/vault/audit?session={session}"), None)
        .await;
    let records: Vec<(String, String, String)> = audit.body["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["tool_call_id"].as_str().unwrap().to_owned(),
                r["what"].as_str().unwrap().to_owned(),
                r["result"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    for expected in [
        ("fill-login", "fill", "filled"),
        ("fill-otp", "otp", "filled"),
    ] {
        let expected = (
            expected.0.to_owned(),
            expected.1.to_owned(),
            expected.2.to_owned(),
        );
        assert!(
            records.contains(&expected),
            "{expected:?} missing from {records:?}"
        );
    }

    // Model input as the sidecar stored it, the durable event log, and every file, log and
    // stderr the harness keeps.
    let transcript = harness.snapshot(&session).to_string();
    let events_text = serde_json::to_string(&all).unwrap();
    assert!(
        transcript.contains("fill-login") && transcript.contains("child-bash"),
        "transcript lacks the probed calls"
    );
    harness.kill().await;
    server.abort();
    let reversed: String = PASSWORD.chars().rev().collect();
    for secret in [PASSWORD, USERNAME, SEED, reversed.as_str()] {
        let forms = [
            secret.to_owned(),
            base64::engine::general_purpose::STANDARD.encode(secret),
            URL_SAFE_NO_PAD.encode(secret),
        ];
        for surface in [&transcript, &events_text] {
            for form in &forms {
                assert!(
                    !surface.contains(form.as_str()),
                    "a vault value reached the model input or event log"
                );
            }
        }
    }
    harness.assert_no_canaries(&[PASSWORD, USERNAME, SEED, &reversed]);
}
