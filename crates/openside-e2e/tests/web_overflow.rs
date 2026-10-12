//! Every main route and conversation surface stays within the viewport at all supported widths.
//! Inner code/table scrollers may be wider inside, but their outer boxes may not escape.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod ui_support;
use serde_json::json;
use ui_support::{Fixture, Ui, KOREAN_TITLE};

const OVERFLOWING: &str = r#"(() => {
  const failures = [];
  if (document.documentElement.scrollWidth > innerWidth)
    failures.push(`document: ${document.documentElement.scrollWidth} > ${innerWidth}`);
  const innerScroller = '.table-scroll, .code-block pre, .tool-body pre, .ticket-tool pre, .subagent-events';
  for (const el of document.querySelectorAll('body *')) {
    if (!el.getClientRects().length || !el.checkVisibility({visibilityProperty:true,contentVisibilityAuto:true}) ||
        el.closest('.sr-only, .ui-offscreen, [hidden]') || el.matches('svg *')) continue;
    const scroller = el.closest(innerScroller);
    if (scroller && scroller !== el) continue;
    const r = el.getBoundingClientRect();
    if (r.width && (r.left < -1 || r.right > innerWidth + 1))
      failures.push(`${el.tagName}.${el.className}: [${r.left.toFixed(1)}, ${r.right.toFixed(1)}] outside [0, ${innerWidth}]`);
  }
  return failures;
})()"#;

async fn check(ui: &mut Ui, place: &str, failures: &mut Vec<String>) {
    ui.cdp.eval("document.fonts.ready.then(() => new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(true)))))").await;
    let result = ui.cdp.eval(OVERFLOWING).await;
    if result != json!([]) {
        failures.push(format!("{place}: {result}"));
    }
}

#[tokio::test]
async fn every_route_and_surface_fits_all_supported_widths() {
    let mut fixture = Fixture::start().await;
    fixture.cards().await;
    let mut ui = Ui::open(
        fixture.executable.clone(),
        &fixture.harness,
        &fixture.token,
        Vec::new(),
    )
    .await;
    let conversation = format!("/sessions/{}", fixture.session);
    let mut failures = Vec::new();
    let widths = [320, 360, 390, 768, 1024, 1280, 1440];
    for width in widths {
        ui.cdp
            .viewport(width, if width <= 390 { 844 } else { 900 }, "light")
            .await;
        ui.goto(&conversation, "document.querySelectorAll('.ticket-pending').length === 5 && !!document.querySelector('.bubble.assistant') && !!document.querySelector('.task')").await;
        check(
            &mut ui,
            &format!("{width}px conversation with login/choice/OTP/approval/confirm"),
            &mut failures,
        )
        .await;
        for ticket in &fixture.tickets {
            let selector = format!("[data-ticket='{}']", ticket["id"].as_str().unwrap());
            ui.cdp
                .eval(&format!(
                    "document.querySelector({}).scrollIntoView({{block:'center'}}); true",
                    json!(selector)
                ))
                .await;
            check(
                &mut ui,
                &format!("{width}px {} card", ticket["kind"]),
                &mut failures,
            )
            .await;
        }
        ui.cdp.eval("document.querySelector('.task').open = true; document.querySelector('.task').scrollIntoView({block:'center'}); true").await;
        check(
            &mut ui,
            &format!("{width}px expanded Tasks panel"),
            &mut failures,
        )
        .await;
        ui.cdp.eval("document.querySelectorAll('.activity:not(.task), .tool-details').forEach(el => el.open = true); true").await;
        check(
            &mut ui,
            &format!("{width}px expanded tool activity"),
            &mut failures,
        )
        .await;
        if let Some(out) = std::env::var_os("OPENSIDE_OVERFLOW_DIR") {
            if width == 390 || width == 1440 {
                let out = std::path::PathBuf::from(out);
                std::fs::create_dir_all(&out).unwrap();
                ui.cdp.eval("document.querySelector('.activity-actions').scrollIntoView({block:'start'}); true").await;
                for theme in ["light", "dark"] {
                    ui.cdp
                        .viewport(width, if width == 390 { 844 } else { 900 }, theme)
                        .await;
                    ui.cdp
                        .capture(&out.join(format!("tool-activity-{theme}-{width}.png")))
                        .await;
                }
                ui.cdp
                    .viewport(width, if width == 390 { 844 } else { 900 }, "light")
                    .await;
            }
        }
        ui.cdp
            .wait("!!document.querySelector('.browser-card-screen, .browser-card-thumb')")
            .await;
        ui.cdp
            .eval(
                "document.querySelector('.browser-card-screen, .browser-card-thumb').click(); true",
            )
            .await;
        ui.cdp
            .wait("!!document.querySelector('.browser-sheet')")
            .await;
        check(
            &mut ui,
            &format!("{width}px expanded browser card"),
            &mut failures,
        )
        .await;
        ui.goto(
            "/",
            "!!document.querySelector('.empty-chat') && !!document.querySelector('.composer')",
        )
        .await;
        check(
            &mut ui,
            &format!("{width}px new chat and code-request dock"),
            &mut failures,
        )
        .await;
        ui.cdp.wait("!!document.querySelector('.otp-dock')").await;
        check(
            &mut ui,
            &format!("{width}px code-request dock"),
            &mut failures,
        )
        .await;
        ui.cdp
            .eval("document.querySelector('.otp-dock-toggle').click(); true")
            .await;
        ui.cdp
            .wait("!!document.querySelector('.otp-dock[data-expanded] .auth-card')")
            .await;
        check(
            &mut ui,
            &format!("{width}px expanded code-request dock"),
            &mut failures,
        )
        .await;
        for (path, title) in [
            ("/settings", "Settings"),
            ("/workspace", "Workspace"),
            ("/skills", "Skills"),
            ("/devices", "Devices"),
            ("/routines", "Routines"),
            ("/memory", "Memory"),
            ("/accounts", "Accounts"),
            ("/vault", "Vault"),
            ("/config", "Configuration"),
        ] {
            ui.goto(path, &format!("Array.from(document.querySelectorAll('.ui-dialog-title')).some(e => e.textContent === {}) && !document.querySelector('[data-starting-style]')", json!(title))).await;
            ui.cdp
                .wait("!document.querySelector('.ui-dialog .loading-row')")
                .await;
            check(&mut ui, &format!("{width}px {path}"), &mut failures).await;
        }
        for step in ["welcome", "agents", "import", "vault", "pair", "done"] {
            ui.goto(
                &format!("/welcome?step={step}"),
                "!!document.querySelector('.onb-body h1')",
            )
            .await;
            ui.cdp
                .wait("!document.querySelector('.onb .loading-row')")
                .await;
            check(&mut ui, &format!("{width}px welcome/{step}"), &mut failures).await;
        }
        ui.goto("/pair", "!!document.querySelector('.pair-page')")
            .await;
        check(&mut ui, &format!("{width}px /pair"), &mut failures).await;
        if width <= 768 {
            ui.goto(
                &conversation,
                "document.querySelectorAll('.ticket-pending').length === 5",
            )
            .await;
            ui.cdp.eval("Array.from(document.querySelectorAll('button')).find(b => b.getAttribute('aria-label') === 'Open conversations' && b.getClientRects().length).click(); true").await;
            ui.cdp.wait(&format!("!!document.querySelector('.ui-drawer') && document.querySelector('.ui-drawer').innerText.includes({}) && !document.querySelector('[data-starting-style]')", json!(KOREAN_TITLE))).await;
            check(
                &mut ui,
                &format!("{width}px conversation drawer"),
                &mut failures,
            )
            .await;
            ui.cdp.eval("(() => { const el=document.querySelector('.ui-drawer input[type=search]'); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(el,'example'); el.dispatchEvent(new Event('input',{bubbles:true})); return true; })()").await;
            ui.cdp
                .wait("document.querySelectorAll('.ui-drawer [data-session]').length === 1")
                .await;
            check(&mut ui, &format!("{width}px drawer search"), &mut failures).await;
        }
    }
    ui.browser.shutdown().await.unwrap();
    fixture.harness.kill().await;
    assert!(
        failures.is_empty(),
        "viewport overflow:\n{}",
        failures.join("\n")
    );
}
