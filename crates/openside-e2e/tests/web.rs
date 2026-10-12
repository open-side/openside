//! Web UI acceptance: the built PWA served by the real daemon, driven in headless
//! Chromium, answers an `ask_user` ticket and the real Python sidecar resumes.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{path::PathBuf, time::Duration};

use openside_browser::{Browser, BrowserConfig};
use openside_e2e::Harness;
use serde_json::{json, Value};

const OWNER: &str = "web-e2e";

fn bundle() -> Option<PathBuf> {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
    if !dist.join("index.html").is_file() {
        eprintln!("SKIP: web UI bundle is missing; run pnpm -C web install && pnpm -C web build");
        return None;
    }
    Some(dist.canonicalize().unwrap())
}

fn chromium() -> Option<PathBuf> {
    match openside_config::discover_browser(&openside_config::Config::default()) {
        Ok(path) => Some(path),
        Err(reason) => {
            eprintln!("SKIP: web UI acceptance requires Chromium: {reason}");
            None
        }
    }
}

struct Page<'a> {
    browser: &'a Browser,
    tab: String,
}

impl Page<'_> {
    async fn eval(&self, expression: &str) -> Value {
        self.browser
            .evaluate(&self.tab, expression)
            .await
            .unwrap_or_else(|e| panic!("evaluate {expression}: {e:?}"))
    }

    /// Poll a page predicate; on timeout, fail with the visible page text.
    async fn wait(&self, what: &str, predicate: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if self.eval(predicate).await == true {
                return;
            }
            if tokio::time::Instant::now() > deadline {
                let text = self
                    .eval("document.body ? document.body.innerText : ''")
                    .await;
                panic!("timed out waiting for {what}; page text: {text}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

// React tracks input values through the native setter; assigning `.value`
// directly would be ignored, so test input goes through the prototype setter.
const HELPERS: &str = r#"(() => {
  window.__violations = [];
  document.addEventListener('securitypolicyviolation', (e) => window.__violations.push(e.violatedDirective));
  window.__os = {
    fill(el, value) {
      const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
        : el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
      el.dispatchEvent(new Event(el instanceof HTMLSelectElement ? 'change' : 'input', { bubbles: true }));
    },
    button(scope, text) {
      return Array.from(scope.querySelectorAll('button')).find((b) => b.textContent.trim() === text);
    },
  };
  return true;
})()"#;

#[tokio::test]
async fn web_ui_answers_ask_user_ticket_and_session_resumes() {
    let (Some(dist), Some(executable)) = (bundle(), chromium()) else {
        return;
    };
    let script = json!({"responses":[
        {"parts":[{"type":"tool_call","name":"ask_user","args":{"question":"Choose a color","options":["blue","green"]},"id":"ask-call"}]},
        {"parts":[{"type":"text","text":"blue selected"}]}
    ]});
    let Some(mut harness) = Harness::prepare(env!("CARGO_BIN_EXE_e2e-opensided"), &script) else {
        return;
    };
    harness.append_config(&format!("[ui]\ndir = {}\n", json!(dist)));
    harness.launch().await;
    // These conversation assertions start after the separately tested first-run guide.
    let onboarding = harness
        .request("PUT", "/v1/onboarding", Some(json!({"state":"skipped"})))
        .await;
    assert_eq!(onboarding.status, 200, "{}", onboarding.body);
    let port = std::fs::read_to_string(harness.data_dir.join("opensided.port"))
        .unwrap()
        .trim()
        .to_owned();
    let paired = harness
        .request(
            "POST",
            "/v1/devices/pair",
            Some(json!({"name":"web e2e","scopes":["read","steer","approve"]})),
        )
        .await;
    assert_eq!(paired.status, 201, "{}", paired.body);
    let token = paired.body["token"].as_str().unwrap().to_owned();

    let (browser, _events) = Browser::launch(BrowserConfig::new(executable))
        .await
        .unwrap();
    let url = format!("http://127.0.0.1:{port}/");
    let tab = browser.create_agent_tab(OWNER, &url).await.unwrap();
    let page = Page {
        browser: &browser,
        tab: tab.id.clone(),
    };
    page.wait(
        "token screen",
        "!!document.querySelector('.token-entry input[type=password]')",
    )
    .await;
    page.eval(HELPERS).await;

    page.eval(&format!(
        "(() => {{ const i = document.querySelector('.token-entry input[type=password]'); __os.fill(i, {}); i.form.requestSubmit(); return true }})()",
        json!(token)
    ))
    .await;
    page.wait(
        "conversation list",
        "!!document.querySelector('.session-list')",
    )
    .await;
    assert_eq!(
        page.eval("localStorage.getItem('openside.token') !== null")
            .await,
        true
    );

    // `/` is the new-chat screen: the model picker lists the daemon's configured default, and
    // the first message creates the conversation (titled after it) and opens it.
    page.eval("document.querySelector('.model-picker button').click(); true")
        .await;
    page.wait(
        "configured default model",
        "Array.from(document.querySelectorAll('[role=dialog] [role=radio]')).some((o) => o.textContent === 'Default (script)' && o.getAttribute('aria-checked') === 'true')",
    )
    .await;
    page.eval(
        "(() => { Array.from(document.querySelectorAll('[role=dialog] [role=radio]')).find((o) => o.textContent === 'Default (script)').click(); return true })()",
    )
    .await;
    page.wait(
        "model dialog closed",
        "!document.querySelector('[role=dialog]')",
    )
    .await;
    page.eval(
        "(() => { const f = document.querySelector('.composer'); \
         __os.fill(f.querySelector('textarea'), 'Which color?'); \
         __os.button(f, 'Send').click(); return true })()",
    )
    .await;
    page.wait(
        "session page",
        "location.pathname.startsWith('/sessions/') && !!document.querySelector('main.session .composer')",
    )
    .await;
    let session = page
        .eval("decodeURIComponent(location.pathname.split('/')[2])")
        .await
        .as_str()
        .unwrap()
        .to_owned();
    page.wait(
        "session title from replayed events",
        "document.querySelector('.session-heading')?.textContent === 'Which color?'",
    )
    .await;

    page.wait(
        "ticket card",
        "!!document.querySelector('section.ticket-pending') && !!__os.button(document.querySelector('section.ticket-pending'), 'blue')",
    )
    .await;
    assert_eq!(
        page.eval("document.querySelector('section.ticket-pending .ticket-summary').textContent")
            .await,
        "Choose a color"
    );
    let waiting = harness
        .request("GET", &format!("/v1/sessions/{session}"), None)
        .await;
    assert_eq!(waiting.body["status"], "waiting_human");

    page.eval("__os.button(document.querySelector('section.ticket-pending'), 'blue').click()")
        .await;
    page.wait(
        "resumed final answer",
        "Array.from(document.querySelectorAll('.bubble.assistant[data-final=true]')).some((b) => b.textContent === 'blue selected') \
         && document.querySelector('main.session')?.dataset.status === 'idle'",
    )
    .await;
    page.wait(
        "resolved ticket card",
        "Array.from(document.querySelectorAll('section.ticket .chip')).some((c) => c.textContent === 'Answered: blue')",
    )
    .await;
    assert_eq!(page.eval("window.__violations.length").await, 0);

    let tickets = harness
        .request("GET", "/v1/tickets?status=resolved", None)
        .await;
    let ticket = tickets.body["tickets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|ticket| ticket["session_id"] == session.as_str())
        .expect("resolved ticket");
    assert_eq!(ticket["resolution"]["answer"], "blue");
    assert_eq!(ticket["tool_call_id"], "ask-call");
    let settled = harness
        .request("GET", &format!("/v1/sessions/{session}"), None)
        .await;
    assert_eq!(settled.body["status"], "idle");
    assert_eq!(settled.body["title"], "Which color?");
    let snapshot = harness.snapshot(&session);
    let returns: Vec<_> = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().into_iter().flatten())
        .filter(|part| part["part_kind"] == "tool-return" && part["tool_call_id"] == "ask-call")
        .collect();
    assert_eq!(returns.len(), 1, "deferred tool resumed exactly once");
    assert_eq!(returns[0]["content"]["answer"], "blue");

    browser.shutdown().await.unwrap();
    harness.kill().await;
}
