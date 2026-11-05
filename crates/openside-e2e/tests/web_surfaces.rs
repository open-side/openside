//! Computed-style acceptance for the flat surface language (solid canvas, no blur, plain
//! assistant text) in the web/PWA shell and the real MV3 extension panel. Captures accompany the assertions at the two reference viewports in both themes.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use ui_support::{assert_transient_status, thinking_session, Cdp, Fixture, Ui, OWNER};

fn surface_assertions(page: &str, user: &str, send: &str) -> String {
    format!(
        r#"(() => {{
      const failures=[];
      const style = selector => {{ const el=document.querySelector(selector); if (!el) {{ failures.push(`missing ${{selector}}`); return null; }} return getComputedStyle(el); }};
      const token = (el, name, property) => {{
        const probe=document.createElement('span'); probe.style[property]=`var(${{name}})`; el.append(probe);
        const value=getComputedStyle(probe)[property]; probe.remove(); return value;
      }};
      const pageEl=document.querySelector({page});
      const pageStyle=style({page});
      if (pageStyle && pageStyle.backgroundImage !== 'none') failures.push(`page has a background image: ${{pageStyle.backgroundImage}}`);
      if (pageStyle && pageStyle.backgroundColor !== token(pageEl,'--canvas','backgroundColor')) failures.push(`page background ${{pageStyle.backgroundColor}} is not --canvas`);
      for (const el of document.querySelectorAll('body *')) {{
        const filter=getComputedStyle(el).backdropFilter;
        if (filter && filter !== 'none') failures.push(`${{el.tagName.toLowerCase()}}.${{el.className}} blurs what is behind it: ${{filter}}`);
      }}
      const assistant=document.querySelector('.bubble.assistant');
      const assistantStyle=style('.bubble.assistant');
      if (assistantStyle && assistantStyle.backgroundColor !== 'rgba(0, 0, 0, 0)') failures.push(`assistant background ${{assistantStyle.backgroundColor}} is not transparent`);
      if (assistantStyle && assistantStyle.color !== token(assistant,'--text','color')) failures.push(`assistant foreground ${{assistantStyle.color}} is not --text`);
      const user=document.querySelector({user});
      const userStyle=style({user});
      if (userStyle && userStyle.backgroundColor !== token(user,'--bubble','backgroundColor')) failures.push(`user bubble ${{userStyle.backgroundColor}} is not --bubble`);
      const cards=[...document.querySelectorAll('.oc-card[data-ticket][data-status=pending]')];
      if (cards.length !== 5) failures.push(`expected all five ticket surfaces, got ${{cards.length}}`);
      for (const el of cards) {{
        const s=getComputedStyle(el);
        if (s.backgroundColor !== token(el,'--surface-raised','backgroundColor')) failures.push(`card ${{el.className}} background ${{s.backgroundColor}} is not --surface-raised`);
      }}
      const sendEl=document.querySelector({send});
      const sendStyle=style({send});
      if (sendStyle && sendStyle.backgroundColor !== token(sendEl,'--send-bg','backgroundColor')) failures.push(`send button ${{sendStyle.backgroundColor}} is not --send-bg`);
      return failures;
    }})()"#,
        page = json!(page),
        user = json!(user),
        send = json!(send)
    )
}

#[tokio::test]
async fn web_and_extension_share_flat_surfaces_and_transient_status_pills() {
    let mut fixture = Fixture::start().await;
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let extension = fixture.harness.root().join("extension");
    let build = tokio::process::Command::new("pnpm")
        .args(["-C", "extension", "build"])
        .current_dir(&repo)
        .env("OPENSIDE_EXTENSION_OUTDIR", &extension)
        .env("OPENSIDE_EXTENSION_TEST_MATCHES", "1")
        .output()
        .await
        .expect("pnpm is required");
    assert!(
        build.status.success(),
        "extension build: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(extension.join("manifest.json")).unwrap()).unwrap();
    let out = PathBuf::from(
        std::env::var("OPENSIDE_SURFACES_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/redesign2/surfaces".into()),
    );
    std::fs::create_dir_all(&out).unwrap();
    let mut ui = Ui::open(
        fixture.executable.clone(),
        &fixture.harness,
        &fixture.token,
        vec![extension.canonicalize().unwrap()],
    )
    .await;
    // Discover by manifest identity, not worker filename: Chromium ships other background.js workers.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let extension_origin = loop {
        let targets = ui.cdp.call("Target.getTargets", json!({})).await;
        let mut found = None;
        for target in targets["targetInfos"].as_array().unwrap() {
            if target["type"] != "service_worker"
                || !target["url"]
                    .as_str()
                    .unwrap_or_default()
                    .ends_with("/background.js")
            {
                continue;
            }
            let mut worker = Cdp::attach(&ui.browser, target["targetId"].as_str().unwrap()).await;
            if worker.eval("chrome.runtime.getManifest().name").await == manifest["name"] {
                found = Some(
                    target["url"]
                        .as_str()
                        .unwrap()
                        .trim_end_matches("background.js")
                        .to_owned(),
                );
                break;
            }
        }
        if let Some(origin) = found {
            break origin;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "MV3 worker did not load"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let panel_tab = ui
        .browser
        .create_agent_tab(OWNER, &format!("{extension_origin}panel.html"))
        .await
        .unwrap()
        .id;
    let mut panel = Cdp::attach(&ui.browser, &panel_tab).await;
    panel
        .wait("!!document.querySelector('#sessions') && !!chrome.storage.session")
        .await;
    panel.eval(&format!("(async () => {{ const tabs=await chrome.tabs.query({{}}); const state={}; for (const tab of tabs) state['sessions:'+tab.id]=[{}]; await chrome.storage.session.set(state); return true; }})()", json!({"pairing":{"url":ui.origin,"token":fixture.token}}), json!(fixture.session))).await;
    panel
        .wait("!!document.querySelector('.bubble.assistant')")
        .await;
    panel.eval("const draft=document.querySelector('#message'); draft.value='Surface acceptance draft'; draft.dispatchEvent(new Event('input',{bubbles:true})); true").await;
    fixture.cards().await;
    // The web user bubble is a column wrapper (text bubble plus attachments); the text paragraph is the bubble surface.
    let web = surface_assertions(
        ".shell-content",
        ".bubble.user > p",
        ".composer button[type=submit]",
    );
    let extension = surface_assertions(
        "body.panel",
        ".bubble.user",
        ".composer button[type=submit]",
    );
    let mut failures = Vec::new();
    for theme in ["light", "dark"] {
        for (width, height) in [(390, 844), (1440, 900)] {
            ui.cdp.viewport(width, height, theme).await;
            ui.goto(&format!("/sessions/{}", fixture.session), "document.querySelectorAll('.oc-card[data-ticket][data-status=pending]').length === 5 && !!document.querySelector('.bubble.assistant')").await;
            ui.cdp.eval("document.querySelectorAll('.oc-card[data-ticket] .oc-pill').forEach(b => b.click()); true").await;
            // An empty composer offers no Send to press (absent while a run queues, else disabled);
            // drafts persist per conversation, so clear the one an earlier pass typed first.
            ui.cdp.eval("(() => { const draft=document.querySelector('.composer textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(draft,''); draft.dispatchEvent(new Event('input',{bubbles:true})); return true; })()").await;
            ui.cdp
                .wait("!document.querySelector('.composer button[type=submit]:not(:disabled)')")
                .await;
            // Send only shows (enabled, in --send-bg) once there is a draft, as in the panel.
            ui.cdp.eval("(() => { const draft=document.querySelector('.composer textarea'); Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(draft,'Surface acceptance draft'); draft.dispatchEvent(new Event('input',{bubbles:true})); return true; })()").await;
            ui.cdp
                .wait("!!document.querySelector('.composer button[type=submit]:not(:disabled)')")
                .await;
            let actual = ui.cdp.eval(&web).await;
            if actual != json!([]) {
                failures.push(format!("web {theme} {width}x{height}: {actual}"));
            }
            ui.cdp
                .capture(&out.join(format!("web-{theme}-{width}x{height}.png")))
                .await;
            let thinking = thinking_session(&fixture.harness).await;
            assert_transient_status(&mut ui, &fixture.harness, &thinking).await;
            panel.viewport(width, height, theme).await;
            // Network latency keeps the real connecting state observable until the SSE opens.
            panel.call("Network.enable", json!({})).await;
            panel.call("Network.emulateNetworkConditions", json!({"offline":false,"latency":800,"downloadThroughput":-1,"uploadThroughput":-1})).await;
            panel
                .eval(
                    "(() => { const root = document.querySelector('#sessions'); const label = root.querySelector('.picker-label').textContent; const pick = (text) => { root.querySelector('.picker-button').click(); [...root.querySelectorAll('[role=option]')].find((option) => option.textContent === text).click(); }; pick('New chat'); pick(label); return true; })()",
                )
                .await;
            panel.wait("getComputedStyle(document.querySelector('.connection')).display !== 'none' && document.querySelector('.connection').textContent === 'Connecting…'").await;
            let status = panel
                .eval("getComputedStyle(document.querySelector('.connection')).borderRadius")
                .await;
            assert_eq!(status, "9999px", "extension connecting status is a pill");
            panel.wait("document.querySelector('.connection').textContent === '' && getComputedStyle(document.querySelector('.connection')).display === 'none'").await;
            panel.call("Network.emulateNetworkConditions", json!({"offline":false,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1})).await;
            panel.wait("document.querySelectorAll('.oc-card[data-ticket][data-status=pending]').length === 5 && !!document.querySelector('.bubble.assistant')").await;
            panel.eval("document.querySelectorAll('.oc-card[data-ticket] .oc-pill').forEach(b=>b.click()); true").await;
            let actual = panel.eval(&extension).await;
            if actual != json!([]) {
                failures.push(format!("extension {theme} {width}x{height}: {actual}"));
            }
            panel
                .capture(&out.join(format!("extension-{theme}-{width}x{height}.png")))
                .await;
        }
    }
    ui.browser.shutdown().await.unwrap();
    fixture.harness.kill().await;
    assert!(
        failures.is_empty(),
        "surface style regressions:\n{}",
        failures.join("\n")
    );
}
