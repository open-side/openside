//! Native user-profile import through the production external-message handler, with replay.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::{SinkExt, StreamExt};
use openside_browser::{Browser, BrowserConfig};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio_tungstenite::tungstenite::Message;

const EXTENSION_ID: &str = "jneddlbpgmlheplolfonafkbkbkjhcfj";

async fn evaluate(browser: &Browser, tab: &str, script: &str) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match browser.evaluate(tab, script).await {
            Ok(value) => return value,
            Err(error)
                if tokio::time::Instant::now() < deadline
                    && matches!(
                        error,
                        openside_browser::BrowserError::TabNotReady(_)
                            | openside_browser::BrowserError::JavaScript(_)
                            | openside_browser::BrowserError::Cdp { .. }
                    ) =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(error) => panic!("native import evaluation failed: {error}"),
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn command(
    socket: &mut Socket,
    id: u64,
    session: Option<&str>,
    method: &str,
    params: Value,
) -> Value {
    let mut request = json!({"id":id,"method":method,"params":params});
    if let Some(session) = session {
        request["sessionId"] = json!(session);
    }
    socket
        .send(Message::text(request.to_string()))
        .await
        .unwrap();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(20), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Message::Text(text) = frame else { continue };
        let mut reply: Value = serde_json::from_str(&text).unwrap();
        if reply["id"] != id {
            continue;
        }
        assert!(
            reply.get("error").is_none(),
            "capture CDP command failed: {reply}"
        );
        return reply["result"].take();
    }
}

async fn capture_permission_page(browser: &Browser, tab: &str, out: &std::path::Path) {
    let (mut socket, _) = tokio_tungstenite::connect_async(browser.endpoint().ws_url)
        .await
        .unwrap();
    let attached = command(
        &mut socket,
        1,
        None,
        "Target.attachToTarget",
        json!({"targetId":tab,"flatten":true}),
    )
    .await;
    let session = attached["sessionId"].as_str().unwrap();
    let mut id = 1;
    for (size, width, height, mobile) in [("desktop", 1440, 900, false), ("phone", 390, 844, true)]
    {
        id += 1;
        command(
            &mut socket,
            id,
            Some(session),
            "Emulation.setDeviceMetricsOverride",
            json!({"width":width,"height":height,"deviceScaleFactor":1,"mobile":mobile}),
        )
        .await;
        for theme in ["light", "dark"] {
            id += 1;
            command(
                &mut socket,
                id,
                Some(session),
                "Emulation.setEmulatedMedia",
                json!({"features":[{"name":"prefers-color-scheme","value":theme}]}),
            )
            .await;
            evaluate(browser, tab, "(async()=>{await document.fonts.ready;const {promise,resolve}=Promise.withResolvers();requestAnimationFrame(()=>requestAnimationFrame(resolve));await promise;return true})()").await;
            assert_eq!(
                evaluate(
                    browser,
                    tab,
                    "document.documentElement.scrollWidth<=window.innerWidth"
                )
                .await,
                true
            );
            id += 1;
            let capture = command(&mut socket, id, Some(session), "Page.captureScreenshot", json!({"format":"png","captureBeyondViewport":false,"clip":{"x":0,"y":0,"width":width,"height":height,"scale":1}})).await;
            std::fs::write(
                out.join(format!("permissions-{size}-{theme}.png")),
                STANDARD.decode(capture["data"].as_str().unwrap()).unwrap(),
            )
            .unwrap();
        }
    }
    let _ = socket.close(None).await;
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn patched_user_profile_import_appears_in_native_bookmark_history_and_cookie_apis() {
    let Some(executable) = std::env::var_os("OPENSIDE_PATCHED_BROWSER") else {
        eprintln!("SKIP native import E2E: set OPENSIDE_PATCHED_BROWSER");
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let extension = root.path().join("extension");
    let built = tokio::process::Command::new("pnpm")
        .args(["-C", "extension", "build"])
        .current_dir(&repo)
        .env("OPENSIDE_EXTENSION_OUTDIR", &extension)
        .env("OPENSIDE_EXTENSION_TEST_MATCHES", "1")
        .output()
        .await
        .unwrap();
    assert!(
        built.status.success(),
        "extension build: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let batch = json!({"id":"cccccccccccccccccccccccccccccccc","source":{"id":"chrome:Default","brand":"chrome","name":"Personal"},"bookmarks":[{"url":format!("{origin}/saved"),"title":"Native saved page","folder":["Work","Reference"],"added_at_ms":1},{"url":"file:///tmp/import-fixture.txt","title":"Local file","folder":[]},{"url":"ftp://example.test/pub/fixture.txt","title":"FTP file","folder":[]},{"url":"chrome://settings/","title":"Internal","folder":[]}],"history":[{"url":format!("{origin}/past"),"title":"Past","visited_at_ms":1}],"cookies":[{"url":format!("{origin}/"),"name":"import_fixture","value":"synthetic-cookie-value","domain":null,"path":"/","secure":false,"httpOnly":true,"sameSite":"lax","expirationDate":expiry,"partitionKey":null}]});
    let payload = std::sync::Arc::new(batch.to_string());
    let consumed = std::sync::Arc::new(parking_lot::Mutex::new(std::collections::BTreeSet::new()));
    let server = tokio::spawn(async move {
        let app = Router::new().route("/", get(|| async { Html("<!doctype html><title>Import verification</title><main>Native import fixture</main>") }))
            .route("/v1/pairing/exchange", post(|headers: HeaderMap, Json(request): Json<Value>| async move {
                let mut response_headers = HeaderMap::new();
                response_headers.insert("content-type", "application/json".parse().unwrap());
                let code = request["code"].as_str().unwrap_or("");
                if code.is_empty() {
                    return (StatusCode::OK, response_headers, json!({"origin":headers.get("origin").and_then(|value| value.to_str().ok())}).to_string());
                }
                assert_eq!(headers.get("origin").and_then(|value| value.to_str().ok()), Some("chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj"));
                let genuine = code == "legit-continuity";
                let replacement = json!({"device_id":if genuine {"c".repeat(32)}else{"d".repeat(32)},"token":if genuine {"new-device-token"}else{"attacker-token"},"scopes":["read"],"kind":"person_device","import_secret":if genuine {"c".repeat(64)}else{"d".repeat(64)}});
                let payload = replacement.to_string();
                let proof = if genuine {
                    let device = headers["x-openside-import-device"].to_str().unwrap();
                    let nonce = headers["x-openside-import-nonce"].to_str().unwrap();
                    assert_eq!(device, "a".repeat(32));
                    assert_eq!(nonce.len(), 64);
                    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &[0xbb; 32]);
                    let mut mac = ring::hmac::Context::with_key(&key);
                    mac.update(b"openside-import-pairing-v1\n");
                    mac.update(device.as_bytes()); mac.update(nonce.as_bytes());
                    mac.update(ring::digest::digest(&ring::digest::SHA256, payload.as_bytes()).as_ref());
                    hex(mac.sign().as_ref())
                } else { "0".repeat(64) };
                response_headers.insert("x-openside-import-proof", proof.parse().unwrap());
                (StatusCode::CREATED, response_headers, payload)
            }))
            .route("/v1/extension/import/{ticket}", get(move |Path(ticket): Path<String>, headers: HeaderMap| {
                let payload = payload.clone();
                let consumed = consumed.clone();
                async move {
                    if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some("Bearer fixture-device-token") || !consumed.lock().insert(ticket.clone()) { return Err(StatusCode::FORBIDDEN); }
                    let hash = hex(ring::digest::digest(&ring::digest::SHA256, payload.as_bytes()).as_ref());
                    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &[0xbb; 32]);
                    let signature = hex(ring::hmac::sign(&key, format!("{ticket}cccccccccccccccccccccccccccccccc{hash}").as_bytes()).as_ref());
                    Ok::<_, StatusCode>(Json(json!({"ticket":ticket,"batch_id":"cccccccccccccccccccccccccccccccc","payload":payload.as_str(),"signature":signature})))
                }
            }));
        axum::serve(listener, app).await.unwrap();
    });
    let mut config = BrowserConfig::new(executable);
    config.profile_dir = Some(root.path().join("user-browser-profile"));
    config.extensions = vec![extension];
    let (browser, _events) = Browser::launch(config).await.unwrap();
    async {
        let page = browser.create_agent_tab("import-fixture", &origin).await.unwrap();
        let extension_page = browser.create_agent_tab("import-fixture", &format!("chrome-extension://{EXTENSION_ID}/options.html")).await.unwrap();
        let native_origin = evaluate(&browser, &extension_page.id, &format!("fetch({},{{method:'POST',headers:{{'Content-Type':'application/json'}},body:'{{}}'}}).then(reply=>reply.json()).then(reply=>reply.origin)", json!(format!("{origin}/v1/pairing/exchange")))).await;
        assert_eq!(native_origin, format!("chrome-extension://{EXTENSION_ID}"));
        let session_pair = json!({"pairing":{"url":origin,"token":"fixture-device-token","device":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}});
        let local_pair = json!({"importPairing":{"device":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","secret":"b".repeat(64),"origin":origin}});
        evaluate(&browser, &extension_page.id, &format!("(async()=>{{await chrome.storage.session.set({session_pair});await chrome.storage.local.set({local_pair});return true}})()")).await;
        let request = |number: u32, kind: &str| format!("new Promise(resolve => chrome.runtime.sendMessage({},{}, reply => resolve(chrome.runtime.lastError ? {{error:chrome.runtime.lastError.message}} : reply)))", json!(EXTENSION_ID), json!({"type":kind,"ticket":format!("{number:064x}")}));
        let script = request(1, "openside.import.apply");
        let first = evaluate(&browser, &page.id, &script).await;
        assert_eq!(first["ok"], true, "native apply reply: {first}");
        assert_eq!(first["bookmarks"], 3);
        assert_eq!(first["skipped"], 1);
        assert_eq!(first["history"], 1);
        let rejected_replay = evaluate(&browser, &page.id, &script).await;
        assert_eq!(rejected_replay["ok"], false, "single-use ticket was replayed");
        let replay = evaluate(&browser, &page.id, &request(2, "openside.import.apply")).await;
        assert_eq!(replay["ok"], true);
        assert_eq!(replay["bookmarks"], 0);
        assert_eq!(replay["history"], 0);
        let observed = evaluate(&browser, &extension_page.id, &format!("(async () => ({{bookmarks:await chrome.bookmarks.search({{url:{}}}),local:await chrome.bookmarks.search({{url:'file:///tmp/import-fixture.txt'}}),ftp:await chrome.bookmarks.search({{url:'ftp://example.test/pub/fixture.txt'}}),history:await chrome.history.search({{text:{},startTime:0,maxResults:100}}),visits:await chrome.history.getVisits({{url:{}}}),cookies:await chrome.cookies.getAll({{url:{}}}),tree:await chrome.bookmarks.getTree()}}))()", json!(format!("{origin}/saved")), json!(format!("{origin}/past")), json!(format!("{origin}/past")), json!(format!("{origin}/")))).await;
        assert_eq!(observed["bookmarks"].as_array().unwrap().len(), 1);
        assert_eq!(observed["local"].as_array().unwrap().len(), 1);
        assert_eq!(observed["ftp"].as_array().unwrap().len(), 1);
        assert_eq!(observed["history"].as_array().unwrap().len(), 1);
        assert_eq!(observed["visits"].as_array().unwrap().len(), 1);
        assert!(observed["history"][0]["lastVisitTime"].as_f64().unwrap() > 1.0);
        let cookie = observed["cookies"].as_array().unwrap().iter().find(|cookie| cookie["name"] == "import_fixture").unwrap();
        assert_eq!(cookie["value"], "synthetic-cookie-value");
        assert_eq!(cookie["hostOnly"], true);
        assert_eq!(cookie["httpOnly"], true);
        assert_eq!(cookie["sameSite"], "lax");
        assert_eq!(cookie["expirationDate"], expiry);
        assert!(observed["tree"].to_string().contains("Imported from Chrome (Personal)"));
        assert!(observed["tree"].to_string().contains("Reference"));
        let permission = evaluate(&browser, &page.id, &request(3, "openside.import.permission.open")).await;
        assert_eq!(permission["ok"], true);
        assert_eq!(permission["granted"], true);
        let permission_page = browser.create_agent_tab("import-fixture", &format!("chrome-extension://{EXTENSION_ID}/import-permissions.html#{}", permission["id"].as_str().unwrap())).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while evaluate(&browser, &permission_page.id, "!!document.querySelector('#allow')&&!document.querySelector('#allow').disabled").await != true {
            assert!(tokio::time::Instant::now() < deadline, "permission page did not load its exact hosts");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let out = std::env::var_os("OPENSIDE_IMPORT_DELIVERY_EVIDENCE").map(PathBuf::from);
        if let Some(out) = out.as_ref() {
            std::fs::create_dir_all(out).unwrap();
            capture_permission_page(&browser, &permission_page.id, out).await;
        }
        if let Some(out) = out.as_ref() {
            let connect_id = "00000000-0000-4000-8000-000000000001";
            evaluate(&browser, &extension_page.id, &format!("(async()=>{{await chrome.storage.session.set({{'importConnect:{connect_id}':{{origin:{},allowed:null}}}});return true}})()", json!(origin))).await;
            let connect = browser.create_agent_tab("import-fixture", &format!("chrome-extension://{EXTENSION_ID}/import-connect.html#{connect_id}")).await.unwrap();
            let directory = out.join("connect");
            std::fs::create_dir_all(&directory).unwrap();
            capture_permission_page(&browser, &connect.id, &directory).await;
        }
        evaluate(&browser, &permission_page.id, "document.querySelector('#allow').click();true").await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while evaluate(&browser, &permission_page.id, "document.querySelector('#status').textContent.startsWith('Site access allowed')").await != true {
            assert!(tokio::time::Instant::now() < deadline, "native site permission request did not complete");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let consent_count = evaluate(&browser, &extension_page.id, "chrome.tabs.query({}).then(tabs=>tabs.filter(tab=>tab.url?.includes('/import-connect.html#')).length)").await;
        evaluate(&browser, &extension_page.id, "(async()=>{await chrome.storage.session.remove('pairing');return true})()").await;
        let pair_request = |code: &str| format!("window.continuityReply=null;chrome.runtime.sendMessage({}, {}, reply=>{{window.continuityReply=chrome.runtime.lastError?{{error:chrome.runtime.lastError.message}}:reply}});true", json!(EXTENSION_ID), json!({"type":"openside.desktop.pair","code":code}));
        evaluate(&browser, &page.id, &pair_request("legit-continuity")).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let legitimate = loop {
            let reply = evaluate(&browser, &page.id, "window.continuityReply").await;
            if !reply.is_null() { break reply; }
            assert_eq!(evaluate(&browser, &extension_page.id, "chrome.tabs.query({}).then(tabs=>tabs.filter(tab=>tab.url?.includes('/import-connect.html#')).length)").await, consent_count, "valid retained-key proof unexpectedly prompted");
            assert!(tokio::time::Instant::now() < deadline, "authenticated continuity did not finish");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(legitimate["ok"], true);
        assert_eq!(evaluate(&browser, &extension_page.id, "(async()=>{const kept=(await chrome.storage.local.get('importPairing')).importPairing;return kept.device==='c'.repeat(32)&&kept.secret==='c'.repeat(64)})()").await, true);
        evaluate(&browser, &extension_page.id, "(async()=>{await chrome.storage.session.remove('pairing');return true})()").await;
        evaluate(&browser, &page.id, &pair_request("attacker-listener")).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let connect_url = loop {
            let url = evaluate(&browser, &extension_page.id, "chrome.tabs.query({}).then(tabs=>tabs.find(tab=>tab.url?.includes('/import-connect.html#')&&!tab.url.endsWith('#00000000-0000-4000-8000-000000000001'))?.url||null)").await;
            if let Some(url) = url.as_str() { break url.to_owned(); }
            assert!(tokio::time::Instant::now() < deadline, "forged listener did not require fresh consent");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(evaluate(&browser, &extension_page.id, "(async()=>{const kept=(await chrome.storage.local.get('importPairing')).importPairing;return kept.secret==='c'.repeat(64)&&!(await chrome.storage.session.get('pairing')).pairing})()").await, true);
        let warning_page = browser.create_agent_tab("import-fixture", &connect_url).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while evaluate(&browser, &warning_page.id, "!!document.querySelector('#warning')&&!document.querySelector('#warning').hidden").await != true {
            assert!(tokio::time::Instant::now() < deadline, "continuity warning did not render");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(evaluate(&browser, &warning_page.id, "document.querySelector('#warning').textContent").await.as_str().unwrap().contains("could not prove it's the same app you paired before"));
        if let Some(out) = out.as_ref() {
            let directory = out.join("continuity-failed");
            std::fs::create_dir_all(&directory).unwrap();
            capture_permission_page(&browser, &warning_page.id, &directory).await;
        }
        evaluate(&browser, &warning_page.id, "document.querySelector('#decline').click();true").await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let declined = loop {
            let reply = evaluate(&browser, &page.id, "window.continuityReply").await;
            if !reply.is_null() { break reply; }
            assert!(tokio::time::Instant::now() < deadline, "declined continuity did not finish");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(declined["ok"], false);
        assert_eq!(evaluate(&browser, &extension_page.id, "(async()=>{const kept=(await chrome.storage.local.get('importPairing')).importPairing;return kept.device==='c'.repeat(32)&&kept.secret==='c'.repeat(64)&&!(await chrome.storage.session.get('pairing')).pairing})()").await, true);
        if let Some(out) = out {
            let mut safe = observed;
            for cookie in safe["cookies"].as_array_mut().unwrap() { cookie["value"] = json!("[REDACTED]"); }
            std::fs::write(out.join("native-user-profile.json"), serde_json::to_vec_pretty(&json!({"first":first,"replay":replay,"rejected_ticket_replay":rejected_replay,"observed":safe,"permission_request":permission,"allow_completed":true,"continuity":{"legitimate_silent":true,"forged_listener_prompted":true,"declined_old_key_unchanged":true}})).unwrap()).unwrap();
        }
    }.await;
    browser.shutdown().await.unwrap();
    server.abort();
    let _ = server.await;
}
