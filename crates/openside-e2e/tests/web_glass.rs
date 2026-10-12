//! Computed-style acceptance for the web/PWA shell and the real MV3 extension panel.
//! Captures accompany the assertions at the two reference viewports in both themes.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use ui_support::{assert_transient_status, thinking_session, Cdp, Fixture, Ui, OWNER};

fn glass_assertions(
    backdrop: &str,
    header: &str,
    assistant: &str,
    cards: &str,
    primary: &str,
) -> String {
    format!(
        r#"(() => {{
      const failures=[];
      const style = selector => {{ const el=document.querySelector(selector); if (!el) {{ failures.push(`missing ${{selector}}`); return null; }} return getComputedStyle(el); }};
      const token = (el, name, property) => {{
        const probe=document.createElement('span'); probe.style[property]=`var(${{name}})`; el.append(probe);
        const value=getComputedStyle(probe)[property]; probe.remove(); return value;
      }};
      const page=style({backdrop});
      if (page && !page.backgroundImage.includes('gradient')) failures.push(`backdrop is ${{page.backgroundImage}}`);
      for (const selector of [{header}, '.composer']) {{
        const s=style(selector);
        if (s && !s.backdropFilter.includes('blur(')) failures.push(`${{selector}} has no blur: ${{s.backdropFilter}}`);
      }}
      const bubble=style({assistant});
      const el=document.querySelector({assistant});
      if (bubble && bubble.backgroundColor !== token(el,'--glass-strong-bg','backgroundColor')) failures.push(`assistant background ${{bubble.backgroundColor}}`);
      if (bubble && bubble.color !== token(el,'--glass-strong-fg','color')) failures.push(`assistant foreground ${{bubble.color}}`);
      const cards=[...document.querySelectorAll({cards})];
      if (cards.length !== 5) failures.push(`expected all five ticket surfaces, got ${{cards.length}}`);
      for (const el of cards) {{
        const s=getComputedStyle(el);
        if (s.backgroundColor !== token(el,'--glass-strong-bg','backgroundColor')) failures.push(`card ${{el.className}} background ${{s.backgroundColor}}`);
      }}
      const buttons=[...document.querySelectorAll({primary})];
      if (buttons.length < 4) failures.push(`missing primary actions: ${{buttons.length}}`);
      for (const button of buttons) {{
        const pill=getComputedStyle(button);
        if (pill.backgroundColor !== token(button,'--pill-primary-bg','backgroundColor')) failures.push(`primary ${{button.className}} background ${{pill.backgroundColor}}`);
        if (pill.color !== token(button,'--pill-primary-fg','color')) failures.push(`primary foreground ${{pill.color}}`);
        if (parseFloat(pill.borderRadius) < parseFloat(pill.height) / 2) failures.push(`primary ${{button.className}} is not a pill: ${{pill.borderRadius}}`);
        if (pill.backgroundColor !== 'rgb(255, 255, 255)') failures.push(`primary is not white: ${{pill.backgroundColor}}`);
      }}
      return failures;
    }})()"#,
        backdrop = json!(backdrop),
        header = json!(header),
        assistant = json!(assistant),
        cards = json!(cards),
        primary = json!(primary)
    )
}

#[tokio::test]
async fn web_and_extension_share_glass_styles_and_transient_status_pills() {
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
        std::env::var("OPENSIDE_GLASS_DIR")
            .unwrap_or_else(|_| "/tmp/openside-qa/polish/LastRows/after".into()),
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
    panel.eval("const draft=document.querySelector('#message'); draft.value='Glass acceptance draft'; draft.dispatchEvent(new Event('input',{bubbles:true})); true").await;
    fixture.cards().await;
    let web = glass_assertions(".shell", ".app-header-heading", ".bubble.assistant", ".ticket-pending .auth-card, .ticket-pending:not(.ticket-auth)", ".composer button[type=submit], .ticket-pending .auth-primary, .ticket-pending .ui-button--primary");
    let extension = glass_assertions(
        "body.panel",
        ".bar select",
        ".bubble.assistant",
        ".ticket .auth-card, .ticket:not(.ticket-auth)",
        ".composer button[type=submit], .ticket .auth-primary, .ticket .button.primary",
    );
    let mut failures = Vec::new();
    for theme in ["light", "dark"] {
        for (width, height) in [(390, 844), (1440, 900)] {
            ui.cdp.viewport(width, height, theme).await;
            ui.goto(&format!("/sessions/{}", fixture.session), "document.querySelectorAll('.ticket-pending').length === 5 && !!document.querySelector('.bubble.assistant')").await;
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
                    "document.querySelector('#sessions').dispatchEvent(new Event('change')); true",
                )
                .await;
            panel.wait("getComputedStyle(document.querySelector('.connection')).display !== 'none' && document.querySelector('.connection').textContent === 'Connecting…'").await;
            let status = panel
                .eval("getComputedStyle(document.querySelector('.connection')).borderRadius")
                .await;
            assert_eq!(status, "999px", "extension connecting status is a pill");
            panel.wait("document.querySelector('.connection').textContent === '' && getComputedStyle(document.querySelector('.connection')).display === 'none'").await;
            panel.call("Network.emulateNetworkConditions", json!({"offline":false,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1})).await;
            panel.wait("document.querySelectorAll('.ticket').length === 5 && !!document.querySelector('.bubble.assistant')").await;
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
        "glass style regressions:\n{}",
        failures.join("\n")
    );
}
