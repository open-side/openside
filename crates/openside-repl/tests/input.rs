//! Input fidelity of the REPL against a real Chromium: actionability and the reported covering
//! element, trusted wheel scrolling, pointer and HTML5 drag and drop, `page.mouse`, the keyboard
//! layout and held keys, fill verification, and Hangul IME composition.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use axum::{response::Html, routing::get, Router};
use openside_browser::{Browser, BrowserConfig};
use openside_repl::{run, Gate, Guard, Limits, Output, Verdict};
use serde_json::json;
use tokio::{sync::Mutex, task::JoinHandle};

const OWNER: &str = "repl-input";
static SERIAL: Mutex<()> = Mutex::const_new(());

const COVER: &str = r#"<!doctype html><title>start</title>
<style>
 body{margin:0;font:16px sans-serif} .row{position:relative;margin:20px;height:50px}
 #scrim{position:absolute;left:0;top:0;width:260px;height:50px;background:rgba(0,0,0,.2)}
 #ghost{pointer-events:none;position:absolute;left:0;top:0;width:260px;height:50px}
 @keyframes slide{from{transform:translateX(0)}to{transform:translateX(500px)}}
 #moving{animation:slide .6s linear forwards;animation-play-state:paused}
</style>
<script>
 const events = [];
 const log = (name) => { events.push(name); document.title = events.join(','); };
</script>
<div class=row><button id=covered onclick="log('covered')">Covered</button>
 <div id=scrim class="scrim big" onclick="log('scrim')"></div></div>
<div class=row><button id=through onclick="log('through')">Through</button><div id=ghost></div></div>
<div class=row><button id=moving onclick="log(this.getAnimations().every(a => a.playState === 'finished') ? 'moved' : 'early')">Moving</button></div>
<div class=row><button id=shift onmousemove="if (!this.dataset.shifted) { this.dataset.shifted = '1'; this.style.transform = 'translateX(150px)'; }" onclick="log('shifted')">Shift</button></div>
<fieldset disabled><button id=fs onclick="log('fieldset')">Fieldset</button></fieldset>
<div aria-disabled="true"><button id=ad onclick="log('aria')">Aria disabled</button></div>
<button id=hidden style="visibility:hidden" onclick="log('hidden')">Hidden</button>
"#;

const SCROLL: &str = r#"<!doctype html><title>scroll</title>
<style>body{margin:0;font:16px sans-serif} #pane{height:200px;overflow:auto;margin:20px;border:1px solid #888}</style>
<script>
 const wheels = { trusted: 0, untrusted: 0 };
 addEventListener('wheel', (event) => {
   event.isTrusted ? wheels.trusted++ : wheels.untrusted++;
   document.body.dataset.wheels = JSON.stringify(wheels);
 }, { passive: true, capture: true });
</script>
<div id=pane><div style="height:2500px"></div>
 <button id=deep onclick="document.title='deep clicked'">Deep</button><div style="height:60px"></div></div>
<div style="height:4000px"></div>
<button id=far onclick="document.title='far clicked'">Far</button><div style="height:300px"></div>
"#;

const DRAG: &str = r#"<!doctype html><title>drag</title>
<style>
 body{margin:0;font:16px sans-serif}
 #card{width:80px;height:40px;background:#cde;margin:20px}
 #zone{width:200px;height:100px;border:2px dashed #888;margin:20px}
 #track{position:relative;width:300px;height:30px;background:#eee;margin:20px}
 #knob{position:absolute;left:0;top:0;width:30px;height:30px;background:#36c}
</style>
<div id=card draggable=true>Card</div><div id=zone>Zone</div><div id=out></div>
<div id=track><div id=knob></div></div>
<script>
 const out = [];
 const note = (text) => { out.push(text); document.getElementById('out').textContent = out.join(' '); };
 card.ondragstart = (e) => { e.dataTransfer.setData('text/plain', 'card'); note('dragstart'); };
 card.ondragend = () => note('dragend');
 zone.ondragenter = (e) => { e.preventDefault(); note('dragenter'); };
 zone.ondragover = (e) => e.preventDefault();
 zone.ondrop = (e) => { e.preventDefault(); note('drop:' + e.dataTransfer.getData('text/plain')); };
 let held = null;
 knob.onmousedown = (e) => { held = { x: e.clientX, left: knob.offsetLeft }; };
 document.onmousemove = (e) => { if (held) knob.style.left = Math.max(0, Math.min(270, held.left + e.clientX - held.x)) + 'px'; };
 document.onmouseup = () => { if (held) note('up@' + knob.offsetLeft); held = null; };
</script>
"#;

const KEYS: &str = r#"<!doctype html><title>keys</title>
<input id=a style="width:300px"><br><textarea id=t rows=3 cols=30></textarea><pre id=log></pre>
<script>
 const lines = [];
 for (const type of ['keydown', 'keyup']) {
   addEventListener(type, (e) => {
     lines.push(`${type}:${e.key}:${e.code}:${(e.ctrlKey ? 'C' : '') + (e.shiftKey ? 'S' : '') + (e.altKey ? 'A' : '') + (e.metaKey ? 'M' : '')}:${e.isTrusted ? 't' : 'u'}${e.repeat ? ':r' : ''}`);
     document.getElementById('log').textContent = lines.join('\n');
   }, true);
 }
</script>
"#;

const FILL: &str = r#"<!doctype html><title>fill</title>
<input id=controlled aria-label=Controlled>
<input id=upper aria-label=Upper>
<input id=reset aria-label=Reset>
<input id=pass type=password aria-label=Pass>
<input id=short maxlength=3 aria-label=Short>
<input id=number type=number aria-label=Number>
<div id=editable contenteditable=true aria-label=Editable style="min-height:20px;border:1px solid #888"></div>
<script>
 let state = '';
 controlled.addEventListener('input', () => { state = controlled.value; controlled.value = state; document.title = 'state:' + state; });
 upper.addEventListener('input', () => { upper.value = upper.value.toUpperCase(); });
 reset.addEventListener('input', () => setTimeout(() => { reset.value = ''; }, 20));
</script>
"#;

const IME: &str = r#"<!doctype html><title>ime</title>
<input id=field aria-label=Field><div id=rich contenteditable=true aria-label=Rich style="min-height:20px;border:1px solid #888"></div><pre id=log></pre>
<script>
 const lines = [];
 const record = (text) => { lines.push(text); document.getElementById('log').textContent = lines.join('\n'); };
 for (const target of [field, rich]) {
   for (const type of ['compositionstart', 'compositionupdate', 'compositionend']) {
     target.addEventListener(type, (e) => record(`${type}:${e.data}`));
   }
   target.addEventListener('beforeinput', (e) => record(`beforeinput:${e.inputType}:${e.isComposing ? 'c' : ''}`));
 }
</script>
"#;

const POLICY: &str = r#"<!doctype html><title>policy</title>
<style>body{margin:0} button{position:absolute;width:120px;height:40px} #safe{left:20px;top:20px} #danger{left:200px;top:20px}</style>
<script>const hits = []; const hit = (n) => { hits.push(n); document.title = hits.join(','); };</script>
<button id=safe onclick="hit('safe')">Safe</button>
<button id=danger onclick="hit('danger')">Delete everything</button>
"#;

const SELECT: &str = r#"<!doctype html><title>select</title>
<select id=choice aria-label=Choice><option value=keep>Keep</option><option value=rename>Rename</option><option value=archive>Archive</option></select>
<select id=unicode aria-label=Unicode><option value=keep>Keep</option><option value=accented>Éclair</option></select>
<select id=duplicate aria-label=Duplicate><option value=one>Same</option><option value=two>Same</option></select>
<select id=blank aria-label=Blank><option value=keep>Keep</option><option value=blank></option></select>
<select id=revert aria-label=Revert onchange="this.value='keep'"><option value=keep>Keep</option><option value=rename>Rename</option></select>
<select id=risky aria-label=Risky><option value=keep>Keep</option><option value=rename selected>Rename</option><option value=other>Remove Delete account</option></select>
<pre id=log></pre>
<script>
 for (const element of document.querySelectorAll('select')) {
   for (const type of ['input', 'change']) {
     element.addEventListener(type, event => {
       document.querySelector('#log').textContent += `${element.id}:${type}:${element.value}:${event.isTrusted}\n`;
     });
   }
 }
</script>
"#;

struct Allow;

impl Guard for Allow {
    fn check(&self, _gate: &Gate<'_>) -> Verdict {
        Verdict::Allow
    }
}

/// Asks before any target whose name holds `Delete`, like the daemon's sensitive keywords.
struct AskDelete;

impl Guard for AskDelete {
    fn check(&self, gate: &Gate<'_>) -> Verdict {
        match gate {
            Gate::Page {
                target: Some(target),
                ..
            } if target.name.contains("Delete") => {
                Verdict::Approval(format!("{} {}", target.action, target.name))
            }
            _ => Verdict::Allow,
        }
    }
}

struct Fixture {
    browser: Arc<Browser>,
    tab: String,
    url: String,
    server: JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Option<Self> {
        let executable = match std::env::var_os("OPENSIDE_FLOW_BROWSER") {
            Some(path) => path.into(),
            None => match openside_config::discover_browser(&openside_config::Config::default()) {
                Ok(path) => path,
                Err(reason) => {
                    eprintln!("SKIP: input tests require Chromium: {reason}");
                    return None;
                }
            },
        };
        let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
            .await
            .expect("launch chromium");
        let browser = Arc::new(browser);
        let tab = browser
            .create_agent_tab(OWNER, "about:blank")
            .await
            .expect("agent tab")
            .id;
        let mut app = Router::new();
        for (path, html) in [
            ("/cover", COVER),
            ("/scroll", SCROLL),
            ("/drag", DRAG),
            ("/keys", KEYS),
            ("/fill", FILL),
            ("/ime", IME),
            ("/policy", POLICY),
            ("/select", SELECT),
        ] {
            app = app.route(path, get(move || async move { Html(html) }));
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Some(Self {
            browser,
            tab,
            url,
            server,
        })
    }

    async fn run_with(&self, code: &str, guard: Arc<dyn Guard>) -> Output {
        let started = std::time::Instant::now();
        let output = run(
            Arc::clone(&self.browser),
            OWNER.into(),
            self.tab.clone(),
            format!("const BASE = {:?};\n{code}", self.url),
            guard,
            Limits::default(),
        )
        .await;
        eprintln!(
            "[{:>5}ms] {code}\n   -> {} {:?}",
            started.elapsed().as_millis(),
            output.return_value,
            output.error
        );
        output
    }

    async fn run(&self, code: &str) -> Output {
        self.run_with(code, Arc::new(Allow)).await
    }

    /// The value of `code`, which must succeed.
    async fn value(&self, code: &str) -> serde_json::Value {
        let output = self.run(code).await;
        assert_eq!(output.error, None, "{output:?}");
        output.return_value
    }

    async fn stop(self) {
        self.server.abort();
        let browser = Arc::try_unwrap(self.browser)
            .ok()
            .expect("REPL runs released the browser");
        browser.shutdown().await.expect("shutdown chromium");
    }
}

macro_rules! fixture {
    ($name:ident) => {
        let _serial = SERIAL.lock().await;
        let Some($name) = Fixture::start().await else {
            return;
        };
    };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_covered_target_is_reported_with_the_element_on_top_and_force_clicks_through() {
    fixture!(f);
    f.value("await page.goto(BASE + '/cover'); 1").await;

    let covered = f
        .run("await page.locator('#covered').click({ timeout: 700 })")
        .await;
    let error = covered.error.expect("covered click must fail");
    assert!(error.contains("receive pointer events"), "{error}");
    assert!(error.contains("<div id=\"scrim\""), "{error}");
    assert!(
        error.contains("scrim big"),
        "names the class of the cover: {error}"
    );
    assert_eq!(f.value("await page.title()").await, json!("start"));

    // A pointer-events:none overlay does not take the click.
    f.value("await page.locator('#through').click(); 1").await;
    assert_eq!(f.value("await page.title()").await, json!("through"));

    // `trial` runs every check and sends nothing; `force` skips them and the cover takes it.
    f.value("await page.locator('#covered').click({ trial: true, force: true }); 1")
        .await;
    assert_eq!(f.value("await page.title()").await, json!("through"));
    f.value("await page.locator('#covered').click({ force: true }); 1")
        .await;
    assert_eq!(f.value("await page.title()").await, json!("through,scrim"));
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actions_wait_for_a_stable_enabled_visible_target() {
    fixture!(f);
    f.value("await page.goto(BASE + '/cover'); 1").await;

    // Start the animation in the same call as the action, not while navigation settles.
    f.value("await page.evaluate(() => { moving.style.animationPlayState = 'running'; }); await page.locator('#moving').click(); 1").await;
    assert_eq!(f.value("await page.title()").await, json!("moved"));
    f.value("await page.locator('#shift').click(); 1").await;
    assert_eq!(f.value("await page.title()").await, json!("moved,shifted"));
    f.value(r#"await page.evaluate(() => {
      const button = document.createElement('button'); button.id = 'resizing'; button.textContent = 'Resize';
      button.style.cssText = 'position:fixed;top:400px;height:40px';
      button.onclick = () => { document.title = button.dataset.done ? 'resized' : 'early resize'; };
      document.body.append(button);
      let frame = 0;
      const resize = () => { const width = 80 + 5 * frame; button.style.width = width + 'px'; button.style.left = (500 - width / 2) + 'px'; if (++frame < 20) requestAnimationFrame(resize); else button.dataset.done = '1'; };
      requestAnimationFrame(resize);
    }); await page.locator('#resizing').click(); 1"#).await;
    assert_eq!(f.value("await page.title()").await, json!("resized"));
    f.value("await page.evaluate(() => { const button=document.querySelector('#through'); button.animate([{color:'red'},{color:'blue'}],{duration:200,iterations:Infinity}); }); await page.locator('#through').click({timeout:1000}); 1").await;
    assert_eq!(
        f.value("await page.title()").await,
        json!("moved,shifted,through")
    );

    for id in ["#fs", "#ad"] {
        let enabled = f
            .value(&format!("await page.locator('{id}').isEnabled()"))
            .await;
        assert_eq!(enabled, json!(false), "{id}");
        let blocked = f
            .run(&format!(
                "await page.locator('{id}').click({{ timeout: 500 }})"
            ))
            .await;
        let error = blocked.error.unwrap();
        assert!(error.contains("enabled"), "{id}: {error}");
    }
    let hidden = f
        .run("await page.locator('#hidden').click({ timeout: 400 })")
        .await;
    assert!(hidden.error.unwrap().contains("visible"));
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scrolling_to_a_target_uses_trusted_wheel_events_inside_nested_scrollers() {
    fixture!(f);
    f.value("await page.goto(BASE + '/scroll'); 1").await;
    f.value("await page.locator('#deep').click(); 1").await;
    assert_eq!(f.value("await page.title()").await, json!("deep clicked"));
    f.value("await page.locator('#far').click(); 1").await;
    assert_eq!(f.value("await page.title()").await, json!("far clicked"));
    let wheels = f
        .value("await page.locator('body').getAttribute('data-wheels')")
        .await;
    let wheels: serde_json::Value = serde_json::from_str(wheels.as_str().unwrap()).unwrap();
    assert!(wheels["trusted"].as_u64().unwrap() > 0, "{wheels}");
    assert_eq!(wheels["untrusted"], json!(0), "{wheels}");
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn html5_drag_and_drop_and_mouse_drags_use_trusted_input() {
    fixture!(f);
    f.value("await page.goto(BASE + '/drag'); 1").await;
    f.value("await page.locator('#card').dragTo(page.locator('#zone')); 1")
        .await;
    let out = f.value("await page.locator('#out').textContent()").await;
    let out = out.as_str().unwrap();
    for event in ["dragstart", "dragenter", "drop:card", "dragend"] {
        assert!(out.contains(event), "{event} missing in {out:?}");
    }

    // The same drag spelled out with the low-level mouse.
    f.value("await page.goto(BASE + '/drag'); 1").await;
    f.value(
        "const c = await page.locator('#card').boundingBox(); const z = await page.locator('#zone').boundingBox();
         await page.mouse.move(c.x + 10, c.y + 10); await page.mouse.down();
         await page.mouse.move(z.x + 50, z.y + 50, { steps: 6 }); await page.mouse.up(); 1",
    )
    .await;
    let out = f.value("await page.locator('#out').textContent()").await;
    assert!(out.as_str().unwrap().contains("drop:card"), "{out}");

    // A plain mouse drag (no HTML5 drag) moves a knob.
    f.value(
        "const k = await page.locator('#knob').boundingBox();
         await page.mouse.move(k.x + 15, k.y + 15); await page.mouse.down();
         await page.mouse.move(k.x + 115, k.y + 15, { steps: 5 }); await page.mouse.up(); 1",
    )
    .await;
    let out = f.value("await page.locator('#out').textContent()").await;
    assert!(out.as_str().unwrap().contains("up@100"), "{out}");

    // `page.dragAndDrop` takes selectors.
    f.value("await page.goto(BASE + '/drag'); await page.dragAndDrop('#card', '#zone'); 1")
        .await;
    let out = f.value("await page.locator('#out').textContent()").await;
    assert!(out.as_str().unwrap().contains("drop:card"), "{out}");
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn page_mouse_clicks_wheel_and_is_judged_by_the_node_under_the_pointer() {
    fixture!(f);
    f.value("await page.goto(BASE + '/policy'); 1").await;
    f.value("const b = await page.locator('#safe').boundingBox(); await page.mouse.click(b.x + 10, b.y + 10); 1")
        .await;
    assert_eq!(f.value("await page.title()").await, json!("safe"));

    for body in [
        "await page.mouse.click(b.x + 10, b.y + 10)",
        "await page.mouse.move(b.x + 10, b.y + 10)",
        "await page.mouse.dblclick(b.x + 10, b.y + 10)",
    ] {
        let denied = f
            .run_with(
                &format!("const b = await page.locator('#danger').boundingBox(); {body}"),
                Arc::new(AskDelete),
            )
            .await;
        assert!(
            denied
                .error
                .as_deref()
                .is_some_and(|error| error.contains("approval required")),
            "{body}: {denied:?}"
        );
    }
    assert_eq!(f.value("await page.title()").await, json!("safe"));

    let outside = f.run("await page.mouse.click(5000, 5000)").await;
    assert!(outside.error.unwrap().contains("viewport"));
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_keyboard_has_a_full_layout_modifiers_and_held_state() {
    fixture!(f);
    f.value("await page.goto(BASE + '/keys'); await page.locator('#a').focus(); 1")
        .await;

    f.value("await page.keyboard.press('Control+Shift+KeyK'); 1")
        .await;
    let log = f.value("await page.locator('#log').textContent()").await;
    let log = log.as_str().unwrap();
    for line in [
        "keydown:Control:ControlLeft:C:t",
        "keydown:Shift:ShiftLeft:CS:t",
        "keydown:K:KeyK:CS:t",
        "keyup:K:KeyK:CS:t",
        "keyup:Shift:ShiftLeft:C:t",
        "keyup:Control:ControlLeft::t",
    ] {
        assert!(log.contains(line), "{line} missing in\n{log}");
    }

    // Shifted symbols come from the shifted layer, with the physical key's code.
    f.value("await page.keyboard.type('Hi, You!'); 1").await;
    assert_eq!(
        f.value("await page.locator('#a').inputValue()").await,
        json!("Hi, You!")
    );
    let log = f.value("await page.locator('#log').textContent()").await;
    assert!(log.as_str().unwrap().contains("keydown:!:Digit1:"), "{log}");

    // A key that is not on the layout is inserted as text.
    f.value("await page.keyboard.type(' é'); 1").await;
    assert_eq!(
        f.value("await page.locator('#a').inputValue()").await,
        json!("Hi, You! é")
    );

    // Held modifiers: Shift+Arrow selects, typing replaces the selection.
    f.value(
        "await page.keyboard.press('Control+a'); await page.keyboard.type('abcdef');
         await page.keyboard.down('Shift'); await page.keyboard.press('ArrowLeft'); await page.keyboard.press('ArrowLeft');
         await page.keyboard.up('Shift'); await page.keyboard.type('X'); 1",
    )
    .await;
    assert_eq!(
        f.value("await page.locator('#a').inputValue()").await,
        json!("abcdX")
    );

    // Word movement and deletion, Home/End.
    f.value(
        "await page.keyboard.press('Control+a'); await page.keyboard.type('one two three');
         await page.keyboard.press('Control+ArrowLeft'); await page.keyboard.press('Control+Backspace');
         await page.keyboard.press('Home'); await page.keyboard.type('>'); await page.keyboard.press('End'); await page.keyboard.type('<'); 1",
    )
    .await;
    assert_eq!(
        f.value("await page.locator('#a').inputValue()").await,
        json!(">one three<")
    );

    // Select all and delete; undo restores.
    f.value("await page.keyboard.press('ControlOrMeta+KeyA'); await page.keyboard.press('Backspace'); 1")
        .await;
    assert_eq!(
        f.value("await page.locator('#a').inputValue()").await,
        json!("")
    );
    f.value("await page.keyboard.press('Control+z'); 1").await;
    assert_ne!(
        f.value("await page.locator('#a').inputValue()").await,
        json!("")
    );

    // The keypad types digits with its own code; a key held twice repeats; Alt is reported.
    f.value("await page.locator('#t').focus(); await page.keyboard.press('Numpad5'); 1")
        .await;
    assert_eq!(
        f.value("await page.locator('#t').inputValue()").await,
        json!("5")
    );
    f.value(
        "await page.keyboard.down('KeyG'); await page.keyboard.down('KeyG'); await page.keyboard.up('KeyG');
         await page.keyboard.press('Alt+KeyX'); 1",
    )
    .await;
    let log = f.value("await page.locator('#log').textContent()").await;
    let log = log.as_str().unwrap();
    assert!(
        log.contains("keydown:Numpad5") || log.contains("keydown:5:Numpad5"),
        "{log}"
    );
    assert!(log.contains("keydown:g:KeyG::t:r"), "{log}");
    assert!(log.contains("keydown:x:KeyX:A:t"), "{log}");

    // Unknown names fail before anything is sent.
    let unknown = f.run("await page.keyboard.press('Hyper+a')").await;
    assert!(unknown.error.unwrap().contains("unknown key modifier"));
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fill_is_verified_and_tells_a_reset_from_a_normalization() {
    fixture!(f);
    f.value("await page.goto(BASE + '/fill'); 1").await;

    // A controlled input that writes its state back keeps the value.
    f.value("await page.getByLabel('Controlled').fill('hello world'); 1")
        .await;
    assert_eq!(
        f.value("await page.title()").await,
        json!("state:hello world")
    );
    f.value("await page.getByLabel('Controlled').fill(''); 1")
        .await;
    assert_eq!(
        f.value("await page.getByLabel('Controlled').inputValue()")
            .await,
        json!("")
    );

    // A page that rewrites the text is reported, not failed: the field holds what the page made.
    let normalized = f.value("await page.getByLabel('Upper').fill('abc')").await;
    assert_eq!(normalized["value"], json!("ABC"), "{normalized}");
    assert_eq!(
        f.value("await page.getByLabel('Upper').inputValue()").await,
        json!("ABC")
    );

    // A field the page empties again did not take the value.
    let reset = f.run("await page.getByLabel('Reset').fill('typed')").await;
    let error = reset
        .error
        .expect("fill must fail when the page resets the field");
    assert!(error.contains("reset"), "{error}");

    // Truncation by maxlength and sanitizing by type are normalizations too.
    let short = f
        .value("await page.getByLabel('Short').fill('abcdef')")
        .await;
    assert_eq!(short["value"], json!("abc"), "{short}");
    let number = f.run("await page.getByLabel('Number').fill('abc')").await;
    assert!(
        number.error.is_some() || number.return_value["value"] == json!(""),
        "{number:?}"
    );

    // A password is verified without its text reaching the script or an error.
    let ok = f
        .value("await page.getByLabel('Pass').fill('s3cret-value'); 1")
        .await;
    assert_eq!(ok, json!(1));
    f.value("await page.getByLabel('Editable').fill('rich text'); 1")
        .await;
    assert_eq!(
        f.value("await page.getByLabel('Editable').textContent()")
            .await,
        json!("rich text")
    );
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn korean_text_is_composed_with_real_composition_events() {
    fixture!(f);
    f.value("await page.goto(BASE + '/ime'); 1").await;
    f.value("await page.getByLabel('Field').pressSequentially('한글 입력', { ime: true }); 1")
        .await;
    assert_eq!(
        f.value("await page.getByLabel('Field').inputValue()").await,
        json!("한글 입력")
    );
    let log = f.value("await page.locator('#log').textContent()").await;
    let log = log.as_str().unwrap();
    for line in [
        "compositionstart:",
        "compositionupdate:ㅎ",
        "compositionupdate:하",
        "compositionupdate:한",
        "compositionupdate:ㄱ",
        "compositionupdate:그",
        "compositionupdate:글",
        "compositionend:한",
        "compositionend:글",
        "beforeinput:insertCompositionText:c",
    ] {
        assert!(log.contains(line), "{line} missing in\n{log}");
    }

    // The low-level pieces: a preedit stays uncommitted until text is inserted.
    f.value(
        "await page.goto(BASE + '/ime'); await page.getByLabel('Field').focus();
         await page.keyboard.imeSetComposition('ㅇ'); await page.keyboard.imeSetComposition('아');
         await page.keyboard.insertText('아'); 1",
    )
    .await;
    assert_eq!(
        f.value("await page.getByLabel('Field').inputValue()").await,
        json!("아")
    );

    // Contenteditable editors see the same events; mixed text keeps its Latin and digits.
    f.value("await page.getByLabel('Rich').pressSequentially('안녕하세요 닭 abc 123', { ime: true }); 1")
        .await;
    assert_eq!(
        f.value("await page.getByLabel('Rich').textContent()").await,
        json!("안녕하세요 닭 abc 123")
    );
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_option_commits_and_reports_only_trusted_input_and_change_events() {
    fixture!(f);
    assert_eq!(
        f.value("await page.goto(BASE + '/select'); const picked = await page.locator('#choice').selectOption('rename'); [picked, await page.locator('#choice').inputValue()]").await,
        json!([["rename"], "rename"]),
    );
    assert_eq!(
        f.value("const picked = await page.locator('#choice').selectOption('keep'); [picked, await page.locator('#choice').inputValue()]").await,
        json!([["keep"], "keep"]),
    );
    assert_eq!(
        f.value("const picked = await page.locator('#unicode').selectOption('accented'); [picked, await page.locator('#unicode').inputValue()]").await,
        json!([["accented"], "accented"]),
    );
    let log = f.value("await page.locator('#log').textContent()").await;
    let log = log.as_str().unwrap();
    assert!(log.contains("choice:input:rename:true"), "{log}");
    assert!(log.contains("choice:change:rename:true"), "{log}");
    assert!(log.contains("unicode:input:accented:true"), "{log}");
    assert!(!log.contains(":false"), "{log}");
    f.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_option_reports_takeover_when_the_page_reverts_the_committed_choice() {
    fixture!(f);
    let result = f.value(
        "await page.goto(BASE + '/select'); const result = await (async () => { try { await page.locator('#revert').selectOption('rename'); return ['unexpected']; } catch (error) { return [error.name, error.message]; } })(); result"
    ).await;
    assert_eq!(result[0], "select_needs_takeover");
    assert!(
        result[1].as_str().unwrap().contains("did not commit"),
        "{result}"
    );
    assert_eq!(
        f.value("await page.locator('#revert').inputValue()").await,
        json!("keep")
    );
    f.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_option_mac_ambiguous_or_empty_labels_need_takeover_before_input() {
    fixture!(f);
    f.value("await page.goto(BASE + '/select'); 1").await;
    for (selector, value) in [("#duplicate", "two"), ("#blank", "blank")] {
        let result = f.value(&format!(
            "const result = await (async () => {{ try {{ await page.locator({selector:?}).selectOption({value:?}); return 'unexpected'; }} catch (error) {{ return error.name; }} }})(); result"
        )).await;
        assert_eq!(result, "select_needs_takeover");
    }
    assert_eq!(
        f.value("await page.locator('#duplicate').inputValue()")
            .await,
        json!("one")
    );
    assert_eq!(
        f.value("await page.locator('#blank').inputValue()").await,
        json!("keep")
    );
    assert_eq!(
        f.value("await page.locator('#log').textContent()").await,
        json!("")
    );
    f.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn select_option_mac_authorizes_all_typeahead_candidates_before_input() {
    fixture!(f);
    f.value("await page.goto(BASE + '/select'); 1").await;
    let result = f
        .run_with(
            "await page.locator('#risky').selectOption('rename')",
            Arc::new(AskDelete),
        )
        .await;
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("approval required") && error.contains("Delete")),
        "{result:?}"
    );
    assert_eq!(
        f.value("await page.locator('#risky').inputValue()").await,
        json!("rename")
    );
    assert_eq!(
        f.value("await page.locator('#log').textContent()").await,
        json!("")
    );
    f.stop().await;
}
