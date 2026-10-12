//! Benign local fixtures for tab, frame, init-script and renderer-recovery boundaries.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Stop, Verdict};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-confinement";
static SERIAL: Mutex<()> = Mutex::const_new(());
const CHILD: &str = r#"<!doctype html><title>Child</title>
<button id=button onclick="document.title='clicked'">Child button</button>
<input id=chooser type=file>
<script>parent.postMessage({ seed: window.__seed ?? null, raw: window.__raw ?? null }, '*');</script>"#;

struct Allow;
impl Guard for Allow {
    fn check(&self, _: &Gate<'_>) -> Verdict {
        Verdict::Allow
    }
}
struct TopOnly {
    origin: String,
}
impl Guard for TopOnly {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        match gate {
            Gate::Page {
                frame: Some(frame), ..
            }
            | Gate::Script {
                frame: Some(frame), ..
            } if frame.origin != self.origin => Verdict::Deny("child origin denied".into()),
            _ => Verdict::Allow,
        }
    }
}
struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    base: String,
    child: String,
    servers: Vec<JoinHandle<()>>,
}
impl Fixture {
    async fn start() -> Self {
        Self::start_with_timeout(None).await
    }
    async fn start_with_timeout(timeout: Option<Duration>) -> Self {
        let executable = openside_config::discover_browser(&openside_config::Config::default())
            .expect("Chromium required");
        let mut config = BrowserConfig::new(executable);
        if let Some(timeout) = timeout {
            config.command_timeout = timeout;
        }
        let (browser, _) = Browser::launch(config).await.unwrap();
        let browser = Arc::new(browser);
        let tab = browser
            .create_agent_tab(OWNER, "about:blank")
            .await
            .unwrap()
            .id;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let child = format!("http://{}", listener.local_addr().unwrap());
        let child_port = listener.local_addr().unwrap().port();
        let child_server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/child", get(|| async { Html(CHILD) })),
            )
            .await
            .unwrap();
        });
        let top = |url: String| {
            format!(
                r#"<!doctype html><title>Top</title>
<style>body{{margin:0}} #top{{position:fixed;left:10px;top:10px;width:80px;height:40px}} iframe{{position:fixed;left:200px;top:100px;width:400px;height:200px;border:0}}</style>
<button id=top>Top button</button><script>window.childSeeds=[];addEventListener('message',e=>childSeeds.push(e.data));</script>
<iframe name=child src="{url}/child"></iframe>"#
            )
        };
        let same_site = top(child.clone());
        let oopif = top(format!("http://localhost:{child_port}"));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(
                        "/",
                        get(move || {
                            let page = same_site.clone();
                            async move { Html(page) }
                        }),
                    )
                    .route(
                        "/oopif",
                        get(move || {
                            let page = oopif.clone();
                            async move { Html(page) }
                        }),
                    )
                    .route(
                        "/csp",
                        get(|| async {
                            (
                                [(
                                    axum::http::header::CONTENT_SECURITY_POLICY,
                                    "script-src 'self'",
                                )],
                                Html("<!doctype html><title>CSP</title>"),
                            )
                        }),
                    ),
            )
            .await
            .unwrap();
        });
        Self {
            browser,
            tab,
            base,
            child,
            servers: vec![server, child_server],
        }
    }
    async fn run(&self, code: &str, guard: Arc<dyn Guard>, limits: Limits) -> Output {
        run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            format!(
                "const BASE={:?}; const CHILD={:?};\n{code}",
                self.base, self.child
            ),
            guard,
            limits,
        )
        .await
    }
    async fn value(&self, code: &str) -> Value {
        let output = self.run(code, Arc::new(Allow), Limits::default()).await;
        assert_eq!(output.error, None, "{code}: {output:?}");
        output.return_value
    }
    async fn open(&self, path: &str) {
        self.value(&format!("await page.goto(BASE + {path:?}); await page.frame('child').locator('#button').waitFor(); null")).await;
    }
    fn top_only(&self) -> Arc<dyn Guard> {
        Arc::new(TopOnly {
            origin: self.base.clone(),
        })
    }
    async fn stop(self) {
        for server in self.servers {
            server.abort();
        }
        Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL released browser")
            .shutdown()
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn raw_cdp_rejects_global_authority_and_gates_frame_contexts_and_files() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/").await;
    let frames = f.browser.frames(OWNER, &f.tab).await.unwrap();
    let child = frames.iter().find(|frame| frame.name == "child").unwrap();
    assert_eq!(child.host, None, "different ports share the page renderer");
    let context = f
        .browser
        .page_command(
            OWNER,
            &f.tab,
            "Page.createIsolatedWorld",
            json!({"frameId": child.id, "worldName":"fixture"}),
        )
        .await
        .unwrap()["executionContextId"]
        .as_i64()
        .unwrap();
    for code in [
        format!("await page.cdp('Runtime.evaluate', {{contextId:{context},expression:'1'}})"),
        format!("await page.cdp('Runtime.callFunctionOn', {{executionContextId:{context},functionDeclaration:'function() {{ return 1; }}'}})"),
        format!("await page.cdp('Page.navigate', {{frameId:{:?},url:CHILD+'/child'}})", child.id),
        "await page.cdp('Input.dispatchMouseEvent', {type:'mouseWheel',x:220,y:120,deltaY:10,deltaX:0})".into(),
        "await page.cdp('Input.dispatchTouchEvent', {type:'touchEnd',touchPoints:[]})".into(),
        "await page.cdp('Network.getCookies', {urls:[CHILD+'/child']})".into(),
        format!("await page.cdp('Network.loadNetworkResource', {{frameId:{:?},url:CHILD+'/child',options:{{disableCache:false,includeCredentials:false}}}})", frames[0].id),
    ] {
        let output = f.run(&code, f.top_only(), Limits::default()).await;
        assert_eq!(output.stop, Some(Stop::Denied("child origin denied".into())), "{code}: {output:?}");
    }
    let raw = f.run("(await page.cdp('Runtime.evaluate',{expression:'6*7',returnByValue:true})).result.value", f.top_only(), Limits::default()).await;
    assert_eq!(raw.error, None, "{raw:?}");
    assert_eq!(raw.return_value, 42);
    let top = f
        .browser
        .frames(OWNER, &f.tab)
        .await
        .unwrap()
        .into_iter()
        .find(|frame| frame.parent.is_none())
        .unwrap();
    let context = f
        .browser
        .page_command(
            OWNER,
            &f.tab,
            "Page.createIsolatedWorld",
            json!({"frameId":top.id,"worldName":"business-data"}),
        )
        .await
        .unwrap()["executionContextId"]
        .as_i64()
        .unwrap();
    let data=f.run(&format!("(await page.cdp('Runtime.callFunctionOn',{{executionContextId:{context},functionDeclaration:'function(value) {{ return value; }}',arguments:[{{value:{{frameId:'business',targetId:'data'}}}}],returnByValue:true}})).result.value"),f.top_only(),Limits::default()).await;
    assert_eq!(data.error, None, "{data:?}");
    assert_eq!(
        data.return_value,
        json!({"frameId":"business","targetId":"data"})
    );
    let rejected = f.value("const denied=[]; for (const method of ['Target.getTargets','Browser.close','SystemInfo.getInfo','Storage.getCookies','Fetch.disable','WebAuthn.getCredentials','Page.setDownloadBehavior']) { try { await page.cdp(method,{}); } catch(e) { denied.push(e.message.includes('reserved by the host')); } } denied").await;
    assert_eq!(rejected, json!([true, true, true, true, true, true, true]));
    for code in [
        "await page.cdp('Network.getCookies',{urls:['http://unrelated.invalid/']})",
        "await page.cdp('Network.loadNetworkResource',{url:'http://unrelated.invalid/',options:{disableCache:false,includeCredentials:false}})",
        "await page.cdp('CacheStorage.requestEntries',{cacheId:'foreign-cache',skipCount:0,pageSize:1})",
        "await page.cdp('DOMStorage.getDOMStorageItems',{storageId:{storageKey:BASE+'^foreign-partition',isLocalStorage:true}})",
        "await page.cdp('Page.addScriptToEvaluateOnLoad',{scriptSource:'window.__legacy=1'})",
    ] {
        let output = f.run(code, Arc::new(Allow), Limits::default()).await;
        assert!(output.error.is_some(), "{code}: {output:?}");
    }
    struct NoUpload;
    impl Guard for NoUpload {
        fn check(&self, gate: &Gate<'_>) -> Verdict {
            match gate {
                Gate::Upload { .. } => Verdict::Deny("upload denied".into()),
                _ => Verdict::Allow,
            }
        }
    }
    let input = f.value("await page.cdp('DOM.getDocument'); const d=await page.cdp('DOM.getDocument'); (await page.cdp('DOM.querySelectorAll',{nodeId:d.root.nodeId,selector:'iframe'})).nodeIds[0]").await;
    assert!(input.is_number());
    // A host fixture resolves the actual child input; the raw upload must still invoke Upload.
    let document = f
        .browser
        .page_command(
            OWNER,
            &f.tab,
            "DOM.getDocument",
            json!({"depth":-1,"pierce":true}),
        )
        .await
        .unwrap();
    fn file_node(node: &Value) -> Option<u64> {
        if node["nodeName"] == "INPUT" {
            return node["backendNodeId"].as_u64();
        }
        for child in node["children"].as_array().into_iter().flatten() {
            if let Some(found) = file_node(child) {
                return Some(found);
            }
        }
        node.get("contentDocument").and_then(file_node)
    }
    let backend = file_node(&document["root"]).unwrap();
    let path = tempfile::NamedTempFile::new().unwrap();
    let upload = f
        .run(
            &format!(
                "await page.cdp('DOM.setFileInputFiles',{{backendNodeId:{backend},files:[{:?}]}})",
                path.path().to_str().unwrap()
            ),
            Arc::new(NoUpload),
            Limits::default(),
        )
        .await;
    assert_eq!(
        upload.stop,
        Some(Stop::Denied("upload denied".into())),
        "{upload:?}"
    );
    let drag = f.run(&format!("await page.cdp('Input.dispatchDragEvent',{{type:'drop',x:20,y:20,data:{{items:[],dragOperationsMask:1,files:[{:?}]}}}})", path.path().to_str().unwrap()), Arc::new(NoUpload), Limits::default()).await;
    assert_eq!(
        drag.stop,
        Some(Stop::Denied("upload denied".into())),
        "{drag:?}"
    );
    assert_eq!(
        f.value("await page.frame('child').locator('#chooser').evaluate(e=>e.files.length)")
            .await,
        0
    );
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_scripts_skip_denied_child_documents_and_filtered_queries_admit_empty_frames() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/").await;
    let output = f.run("await page.addInitScript('window.__seed=1'); await page.cdp('Page.addScriptToEvaluateOnNewDocument',{source:'window.__raw=2'}); await page.reload(); await page.waitForFunction(()=>childSeeds.length===1); [await page.evaluate(()=>[__seed,__raw]),await page.evaluate(()=>childSeeds[0])]", f.top_only(), Limits::default()).await;
    assert_eq!(output.error, None, "{output:?}");
    assert_eq!(output.return_value, json!([[1,2],{"seed":null,"raw":null}]));
    for code in [
        "await page.frame('child').locator('#absent').count()",
        "await page.frame('child').locator('#button').filter({hasText:'absent'}).count()",
        "await page.locator('iframe').filter({has:page.frame('child').locator('#absent')}).count()",
        "await page.locator('iframe').filter({hasNot:page.frame('child').locator('#absent')}).count()",
    ] {
        let output = f.run(code, f.top_only(), Limits::default()).await;
        assert_eq!(output.stop, Some(Stop::Denied("child origin denied".into())), "{code}: {output:?}");
    }
    let csp = f.run("await page.addInitScript('window.__cspSeed=3'); await page.goto(BASE+'/csp'); await page.evaluate(()=>window.__cspSeed)", f.top_only(), Limits::default()).await;
    assert_eq!(csp.error, None, "{csp:?}");
    assert_eq!(
        csp.return_value, 3,
        "allowed-origin init scripts must work with CSP"
    );
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wheel_and_held_moves_admit_the_receiving_child_origin() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/").await;
    for code in [
        "await page.mouse.move(20,20); await page.evaluate(()=>{const frame=document.querySelector('iframe');frame.style.left='0';frame.style.top='0';document.querySelector('#top').hidden=true;}); await page.mouse.wheel(0,30)",
        "await page.mouse.move(20,20); await page.mouse.down(); await page.mouse.move(220,120)",
    ] {
        f.open("/").await;
        let output = f.run(code, f.top_only(), Limits::default()).await;
        assert_eq!(output.stop, Some(Stop::Denied("child origin denied".into())), "{code}: {output:?}");
    }
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_file_choosers_keep_frame_identity_for_event_and_upload_admission() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/").await;
    let path = tempfile::NamedTempFile::new().unwrap();
    struct ChooserGuard {
        origin: String,
        deny_delivery: bool,
        opened: AtomicBool,
    }
    impl Guard for ChooserGuard {
        fn check(&self, gate: &Gate<'_>) -> Verdict {
            match gate {
                Gate::Script {
                    frame: Some(frame),
                    api: "evaluate",
                    ..
                } if frame.origin != self.origin => {
                    self.opened.store(true, Ordering::Release);
                    Verdict::Allow
                }
                Gate::Page {
                    frame: Some(frame), ..
                } if self.deny_delivery
                    && self.opened.load(Ordering::Acquire)
                    && frame.origin != self.origin =>
                {
                    Verdict::Deny("chooser receiver denied".into())
                }
                Gate::Upload {
                    frame: Some(frame), ..
                } if frame.origin != self.origin => {
                    Verdict::Deny("chooser upload receiver denied".into())
                }
                _ => Verdict::Allow,
            }
        }
    }
    for deny_delivery in [true, false] {
        let guard = Arc::new(ChooserGuard {
            origin: f.base.clone(),
            deny_delivery,
            opened: AtomicBool::new(false),
        });
        let output = f.run(&format!("const [chooser]=await Promise.all([page.waitForEvent('filechooser'),page.frame('child').evaluate(()=>document.querySelector('#chooser').click())]); chooser._raw.frame=null; chooser._raw.host=null; await chooser.setFiles({:?})", path.path().to_str().unwrap()), guard, Limits::default()).await;
        let reason = if deny_delivery {
            "chooser receiver denied"
        } else {
            "chooser upload receiver denied"
        };
        assert_eq!(output.stop, Some(Stop::Denied(reason.into())), "{output:?}");
    }
    assert_eq!(
        f.value("await page.frame('child').locator('#chooser').evaluate(e=>e.files.length)")
            .await,
        0
    );
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandoned_oopif_is_recovered_without_terminating_another_owned_tab() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/oopif").await;
    let sibling = f
        .browser
        .create_agent_tab(OWNER, &format!("{}/", f.base))
        .await
        .unwrap()
        .id;
    let frames = f.browser.frames(OWNER, &f.tab).await.unwrap();
    assert!(frames
        .iter()
        .any(|frame| frame.name == "child" && frame.host.is_some()));
    let started = Instant::now();
    let output = f
        .run(
            "await page.frame('child').evaluate(()=>{for(;;){}})",
            Arc::new(Allow),
            Limits {
                timeout: Duration::from_millis(1500),
                ..Limits::default()
            },
        )
        .await;
    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TimeoutError:")),
        "{output:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "cleanup exceeded its bound"
    );
    assert_eq!(
        f.value("[await page.frame('child').evaluate(()=>1),await page.title()]")
            .await,
        json!([1, "Top"])
    );
    let output = run(
        Arc::clone(&f.browser),
        OWNER.into(),
        sibling,
        "await page.evaluate(()=>7)".into(),
        Arc::new(Allow),
        Limits::default(),
    )
    .await;
    assert_eq!(output.error, None, "{output:?}");
    assert_eq!(output.return_value, 7);
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_protocol_timeout_still_recovers_its_oopif_and_removes_init_scripts() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start_with_timeout(Some(Duration::from_secs(1))).await;
    f.open("/oopif").await;
    let started = Instant::now();
    let output=f.run("await page.addInitScript('window.__oldTask=1'); try {await page.frame('child').evaluate(()=>{for(;;){}});} catch(e) {} 42",Arc::new(Allow),Limits::default()).await;
    assert_eq!(output.error, None, "{output:?}");
    assert_eq!(output.return_value, 42);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "timed-out call recovery exceeded its bound"
    );
    assert_eq!(
        f.value("[await page.frame('child').evaluate(()=>7),await page.title()]")
            .await,
        json!([7, "Top"])
    );
    assert_eq!(f.value("await page.reload(); await page.frame('child').locator('#button').waitFor(); await page.evaluate(()=>typeof __oldTask)").await,"undefined");
    let started = Instant::now();
    let output = f
        .run(
            "try { await page.cdp('Runtime.evaluate', {expression:'for(;;){}'}); } catch(e) {} 43",
            Arc::new(Allow),
            Limits::default(),
        )
        .await;
    assert_eq!(output.error, None, "{output:?}");
    assert_eq!(output.return_value, 43);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(f.value("await page.evaluate(()=>9)").await, 9);
    f.stop().await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_pin_verified_bytes_and_refuse_a_parent_swapped_after_admission() {
    use std::{path::PathBuf, sync::atomic::AtomicUsize};
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/").await;
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(workspace.path().join("parent")).unwrap();
    let source = workspace.path().join("parent/file.txt");
    std::fs::write(&source, b"original").unwrap();
    std::fs::write(outside.path().join("file.txt"), b"substituted").unwrap();
    struct SwapGuard {
        root: PathBuf,
        outside: PathBuf,
        checks: AtomicUsize,
    }
    impl Guard for SwapGuard {
        fn check(&self, gate: &Gate<'_>) -> Verdict {
            if let Gate::Upload { path, .. } = gate {
                if !std::path::Path::new(path).starts_with(&self.root) {
                    return Verdict::Deny("outside workspace".into());
                }
                if self.checks.fetch_add(1, Ordering::AcqRel) == 1 {
                    std::fs::rename(self.root.join("parent"), self.root.join("old")).unwrap();
                    std::os::unix::fs::symlink(&self.outside, self.root.join("parent")).unwrap();
                }
            }
            Verdict::Allow
        }
    }
    let guard = Arc::new(SwapGuard {
        root: workspace.path().to_owned(),
        outside: outside.path().to_owned(),
        checks: AtomicUsize::new(0),
    });
    let failed = f
        .run(
            &format!(
                "await page.frame('child').locator('#chooser').setInputFiles({:?})",
                source.to_str().unwrap()
            ),
            guard,
            Limits::default(),
        )
        .await;
    assert!(failed.error.is_some(), "{failed:?}");
    assert_eq!(
        f.value("await page.frame('child').locator('#chooser').evaluate(e=>e.files.length)")
            .await,
        0
    );
    std::fs::remove_file(workspace.path().join("parent")).unwrap();
    std::fs::rename(
        workspace.path().join("old"),
        workspace.path().join("parent"),
    )
    .unwrap();
    f.value(&format!(
        "await page.frame('child').locator('#chooser').setInputFiles({:?}); null",
        source.to_str().unwrap()
    ))
    .await;
    std::fs::write(&source, b"changed").unwrap();
    assert_eq!(f.value("await page.frame('child').locator('#chooser').evaluate(async e=>[e.files[0].name,await e.files[0].text()])").await,json!(["file.txt","original"]));
    let staged = f.browser.profile_path().join("upload-staging").join(&f.tab);
    assert!(staged.is_dir());
    assert!(!f.browser.close_space(OWNER).await.unwrap());
    assert!(
        !staged.exists(),
        "shared-session end removes staged files too"
    );
    f.value(&format!(
        "await page.frame('child').locator('#chooser').setInputFiles({:?}); null",
        source.to_str().unwrap()
    ))
    .await;
    assert!(
        staged.is_dir(),
        "later valid use can create a fresh generation"
    );
    f.browser.close_agent_tab(OWNER, &f.tab).await.unwrap();
    assert!(!staged.exists(), "tab close drains staged files");
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_abandoned_object_replies_do_not_invalidate_a_new_execution_handle() {
    let _serial = SERIAL.lock().await;
    let f = Fixture::start().await;
    f.open("/").await;
    let output=f.run("await page.evaluateHandle(async()=>{await new Promise(resolve=>setTimeout(resolve,800));return {old:true};})",
        Arc::new(Allow),Limits {timeout:Duration::from_millis(150),..Limits::default()}).await;
    assert!(
        output
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with("TimeoutError:")),
        "{output:?}"
    );
    assert_eq!(f.value("const fresh=await page.evaluateHandle(()=>({fresh:7}));await page.waitForTimeout(1100);const value=await fresh.evaluate(object=>object.fresh);await fresh.dispose();value").await,7);
    f.stop().await;
}
