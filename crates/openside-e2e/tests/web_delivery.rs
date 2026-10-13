//! Visual QA for message delivery feedback (ignored by default): a message sent from the web
//! composer shows Sent, then Read, then bouncing typing dots, then the reply takes over.
//! Drives headless Chromium against the hermetic daemon on a scripted model whose reply is
//! delayed, so every state lasts long enough to capture, and writes timed screenshots plus
//! `timeline.json` (state, milliseconds since the click) to `OPENSIDE_DELIVERY_DIR`
//! (default `/tmp/openside-qa/round2/ChatStates`).
//!
//! Run: `pnpm -C web build && cargo test -p openside-e2e --test web_delivery -- --ignored --nocapture`

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use ui_support::{create, text, Ui};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");

const HELPERS: &str = r#"(() => {
  window.__d = {
    send(value) {
      const form = document.querySelector('.composer');
      const area = form.querySelector('textarea');
      Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(area, value);
      area.dispatchEvent(new Event('input', { bubbles: true }));
      form.querySelector('button[type=submit]:not(:disabled)').click();
      return true;
    },
    state() {
      const last = Array.from(document.querySelectorAll('.timeline > li.bubble.user')).at(-1);
      const receipt = last && last.querySelector('.receipt');
      const typing = !!document.querySelector('.typing');
      const reply = !!document.querySelector('.bubble.assistant');
      if (reply) return 'reply';
      const note = receipt ? receipt.textContent.trim() : 'none';
      return typing ? note + '+typing' : note;
    },
    dots() {
      return Array.from(document.querySelectorAll('.typing-dot')).map((dot) => {
        const s = getComputedStyle(dot);
        return { transform: s.transform, opacity: s.opacity, animation: s.animationName };
      });
    },
  };
  return true;
})()"#;

async fn motion(ui: &mut Ui, reduce: bool) {
    ui.cdp
        .call(
            "Emulation.setEmulatedMedia",
            json!({"features":[{"name":"prefers-reduced-motion","value":if reduce {"reduce"} else {"no-preference"}}]}),
        )
        .await;
}

async fn session_page(ui: &mut Ui, harness: &openside_e2e::Harness, title: &str) {
    let session = create(harness, title).await;
    ui.goto(
        &format!("/sessions/{session}"),
        "!!document.querySelector('.composer textarea')",
    )
    .await;
    ui.cdp.eval(HELPERS).await;
}

/// Sends through the real composer and records every distinct delivery state with a capture.
async fn walk(ui: &mut Ui, out: &Path, label: &str, message: &str) -> Vec<Value> {
    let started = Instant::now();
    ui.cdp.eval(&format!("__d.send({})", json!(message))).await;
    let mut seen: Vec<Value> = Vec::new();
    let mut last = String::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let state = ui
            .cdp
            .eval("__d.state()")
            .await
            .as_str()
            .unwrap()
            .to_owned();
        if state != last {
            let at = started.elapsed().as_millis() as u64;
            let name = format!(
                "{label}-{:02}-{}.png",
                seen.len() + 1,
                state.to_lowercase().replace(['…', ' ', '+'], "-")
            );
            ui.cdp.capture(&out.join(&name)).await;
            seen.push(json!({"state": state, "at_ms": at, "file": name}));
            last = state.clone();
            if state.ends_with("+typing") {
                // Two samples apart show the dots moving (or fading, under reduced motion).
                let first = ui.cdp.eval("__d.dots()").await;
                tokio::time::sleep(Duration::from_millis(350)).await;
                let second = ui.cdp.eval("__d.dots()").await;
                ui.cdp
                    .capture(&out.join(format!("{label}-typing-later.png")))
                    .await;
                seen.push(json!({"state":"typing-dots","first":first,"second":second}));
            }
            if state == "reply" {
                return seen;
            }
        }
        assert!(Instant::now() < deadline, "no reply; saw {seen:?}");
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn states(seen: &[Value]) -> Vec<String> {
    seen.iter()
        .filter_map(|entry| entry["state"].as_str())
        .filter(|state| *state != "typing-dots")
        .map(str::to_owned)
        .collect()
}

fn assert_in_order(seen: &[Value], wanted: &[&str]) {
    let order = states(seen);
    let mut cursor = 0;
    for state in order.iter() {
        if cursor < wanted.len() && state == wanted[cursor] {
            cursor += 1;
        }
    }
    assert_eq!(
        cursor,
        wanted.len(),
        "expected {wanted:?} in order, saw {order:?}"
    );
}

#[tokio::test]
#[ignore = "visual QA harness; see the module docs"]
async fn delivery_states_sent_read_typing_reply() {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    assert!(dist.join("index.html").is_file(), "build web/dist first");
    let executable = openside_config::discover_browser(&openside_config::Config::default())
        .expect("headless Chromium");
    let out = PathBuf::from(
        std::env::var("OPENSIDE_DELIVERY_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/round2/ChatStates".into()),
    );
    std::fs::create_dir_all(&out).unwrap();

    // The delay applies before the model's first words: the "working, nothing to say yet" window.
    let script = |reply: &str| json!({"responses":[text(reply)],"delay_ms":3500});
    let mut harness = openside_e2e::Harness::prepare(DAEMON, &script("Here is the answer."))
        .expect("hermetic daemon prerequisites");
    harness.append_config(&format!(
        "[ui]\ndir = {}\n[browser]\nexecutable = {}\n",
        json!(dist.canonicalize().unwrap()),
        json!(executable)
    ));
    harness.launch().await;
    assert_eq!(
        harness
            .request("PUT", "/v1/onboarding", Some(json!({"state":"completed"})))
            .await
            .status,
        200
    );
    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"delivery-qa","scopes":["read","steer","approve","human_step","admin"]})),
        )
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_owned();
    let mut ui = Ui::open(executable, &harness, &token, Vec::new()).await;
    let mut report = serde_json::Map::new();

    // Desktop, light, full motion: the dots bounce.
    ui.cdp.viewport(1440, 900, "light").await;
    motion(&mut ui, false).await;
    session_page(&mut ui, &harness, "Delivery desktop").await;
    let desktop = walk(
        &mut ui,
        &out,
        "desktop-light",
        "Compare the two pricing pages for me.",
    )
    .await;
    assert_in_order(&desktop, &["Sent", "Read+typing", "reply"]);
    let dots = desktop
        .iter()
        .find(|entry| entry["state"] == "typing-dots")
        .expect("typing frames");
    assert_ne!(
        dots["first"], dots["second"],
        "dots must move between samples under full motion"
    );
    report.insert("desktop-light".into(), json!(desktop));

    // Phone, dark, reduced motion: the same states with a fade instead of movement.
    harness.replace_script(&script("Done. Anything else?"));
    ui.cdp.viewport(390, 844, "dark").await;
    motion(&mut ui, true).await;
    session_page(&mut ui, &harness, "Delivery phone").await;
    let phone = walk(
        &mut ui,
        &out,
        "phone-dark-reduced",
        "Book me a table for two.",
    )
    .await;
    assert_in_order(&phone, &["Sent", "Read+typing", "reply"]);
    let dots = phone
        .iter()
        .find(|entry| entry["state"] == "typing-dots")
        .expect("typing frames");
    assert_eq!(dots["first"][0]["animation"], "typing-pulse", "{dots}");
    report.insert("phone-dark-reduced".into(), json!(phone));

    // A POST the daemon refuses: Not sent with Retry, then the retry goes through.
    harness.replace_script(&script("Sent on the second try."));
    ui.cdp.viewport(1440, 900, "dark").await;
    motion(&mut ui, false).await;
    session_page(&mut ui, &harness, "Delivery failure").await;
    ui.cdp
        .eval(
            r#"(() => {
              window.__realFetch = window.fetch;
              window.fetch = (input, init) => {
                const url = String(input instanceof Request ? input.url : input);
                if (window.__refuse && /\/messages$/.test(url) && ((init && init.method) || 'GET') === 'POST') {
                  return Promise.resolve(new Response(JSON.stringify({code:'internal', message:'The daemon refused the message'}), { status: 500, headers: {'Content-Type':'application/json'} }));
                }
                return window.__realFetch(input, init);
              };
              window.__refuse = true;
              return true;
            })()"#,
        )
        .await;
    ui.cdp
        .eval("__d.send('This one will not go through.')")
        .await;
    ui.cdp
        .wait("!!document.querySelector('.receipt-failed')")
        .await;
    ui.cdp.capture(&out.join("failure-01-not-sent.png")).await;
    ui.cdp.eval("window.__refuse = false; true").await;
    ui.cdp
        .eval("Array.from(document.querySelectorAll('.receipt-action')).find((b) => b.textContent.includes('Retry')).click()")
        .await;
    ui.cdp
        .wait("!document.querySelector('.receipt-failed') && !!document.querySelector('.bubble.assistant')")
        .await;
    ui.cdp
        .capture(&out.join("failure-02-after-retry.png"))
        .await;

    // Reloading mid-run reproduces the state from the event log.
    harness.replace_script(&script("Back after a reload."));
    ui.cdp.viewport(1440, 900, "light").await;
    motion(&mut ui, false).await;
    session_page(&mut ui, &harness, "Delivery reload").await;
    ui.cdp.eval("__d.send('Reload while you work.')").await;
    ui.cdp.wait("__d.state().startsWith('Read')").await;
    let path = ui.cdp.eval("location.pathname").await;
    ui.goto(
        path.as_str().unwrap(),
        "!!document.querySelector('.composer textarea')",
    )
    .await;
    ui.cdp.eval(HELPERS).await;
    ui.cdp.wait("__d.state().startsWith('Read')").await;
    ui.cdp.capture(&out.join("reload-01-restored.png")).await;
    ui.cdp.wait("__d.state() === 'reply'").await;
    report.insert(
        "reload".into(),
        json!("restored from the event log, then replied"),
    );

    std::fs::write(
        out.join("timeline.json"),
        serde_json::to_string_pretty(&Value::Object(report)).unwrap(),
    )
    .unwrap();
}
