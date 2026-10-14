//! B-7 tab-ownership and asleep-tab acceptance, plus the controller half of PP-23 (a force-
//! killed browser is detected and its persistent profile relaunches), against real Chromium
//! with loopback fixtures and temporary profiles only.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use std::time::{Duration, Instant};

use openside_browser::{Action, Browser, BrowserConfig, BrowserError, BrowserEvent, OpenedBy};
use serde_json::json;

use common::{browser_config, event, gone, ready, serial, Server};

fn reference(snapshot: &openside_browser::Snapshot, role: &str, name: &str) -> String {
    snapshot
        .nodes
        .iter()
        .find(|node| node.role == role && node.name == name)
        .unwrap_or_else(|| panic!("missing {role} {name} in snapshot"))
        .reference
        .clone()
        .expect("interactive node must have a reference")
}

/// B-7 ①: an agent closes, navigates, clicks and types only in tabs it opened. A user tab
/// stays readable (snapshot) but every mutation is refused and leaves the page unchanged.
#[tokio::test]
async fn agent_closes_only_its_own_tabs_and_user_tabs_are_read_only() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    let initial = event(&mut events, |event| matches!(event, BrowserEvent::TabOpened { tab } if tab.opened_by == OpenedBy::External)).await;
    let BrowserEvent::TabOpened { tab: user } = initial else {
        unreachable!()
    };
    browser.adopt_user_tab(&user.id).await.unwrap();
    // The user's own page content; the agent never wrote it.
    browser
        .evaluate(
            &user.id,
            "document.body.innerHTML = '<label>Note<input id=note></label><button onclick=\"window.clicked=true\">Send</button>'; window.clicked = false; true",
        )
        .await
        .unwrap();
    let user_snapshot = browser.snapshot(&user.id).await.unwrap();
    assert!(user_snapshot.text.contains("Send"), "user tab is readable");
    let note = reference(&user_snapshot, "textbox", "Note");
    let send = reference(&user_snapshot, "button", "Send");

    let own = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &own).await;
    let other = browser
        .create_agent_tab("other-owner", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &other).await;

    for tab in [&user.id, &other.id] {
        assert!(matches!(
            browser.close_agent_tab("owner", tab).await,
            Err(BrowserError::OwnershipDenied(_))
        ));
        assert!(matches!(
            browser
                .navigate("owner", tab, &server.page("welcome"))
                .await,
            Err(BrowserError::OwnershipDenied(_))
        ));
        assert!(matches!(
            browser
                .page_command("owner", tab, "Page.reload", json!({}))
                .await,
            Err(BrowserError::OwnershipDenied(_))
        ));
    }
    assert!(matches!(
        browser.act("owner", &user.id, &send, Action::Click).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(browser
        .insert_text("owner", &user.id, &note, "agent-typed-canary")
        .await
        .is_err());
    assert_eq!(
        browser
            .evaluate(
                &user.id,
                "[location.href, document.getElementById('note').value, window.clicked]"
            )
            .await
            .unwrap(),
        json!(["about:blank", "", false]),
        "a refused mutation must leave the user tab untouched"
    );

    // The owner keeps full control over its own tab.
    browser
        .navigate("owner", &own.id, &server.page("welcome"))
        .await
        .unwrap();
    browser.close_agent_tab("owner", &own.id).await.unwrap();
    event(
        &mut events,
        |event| matches!(event, BrowserEvent::TabClosed { tab, .. } if tab.id == own.id),
    )
    .await;
    let tabs = browser.tabs().await;
    assert!(tabs
        .iter()
        .any(|tab| tab.id == user.id && tab.opened_by == OpenedBy::User));
    assert!(tabs.iter().any(|tab| tab.id == other.id));

    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

/// B-7 ③: a discarded or unloaded tab is never attached; every operation that needs the page
/// fails fast with the distinct `TabAsleep` error, far inside the 12-second attach deadline.
#[tokio::test]
async fn asleep_tabs_are_not_attached_and_fail_with_tab_asleep_within_twelve_seconds() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    assert_eq!(config.attach_timeout, Duration::from_secs(12));
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let save = reference(&browser.snapshot(&tab.id).await.unwrap(), "button", "Save");
    for (discarded, unloaded) in [(true, false), (false, true)] {
        browser
            .mark_asleep(&tab.id, discarded, unloaded)
            .await
            .unwrap();
        browser.record_protocol(true);
        let started = Instant::now();
        let attach = browser.attach(&tab.id).await;
        let snapshot = browser.snapshot(&tab.id).await;
        let act = browser.act("owner", &tab.id, &save, Action::Click).await;
        let navigate = browser
            .navigate("owner", &tab.id, &server.page("welcome"))
            .await;
        let elapsed = started.elapsed();
        let protocol = browser.take_protocol_log();
        browser.record_protocol(false);
        assert!(matches!(&attach, Err(BrowserError::TabAsleep(id)) if id == &tab.id));
        assert!(matches!(snapshot, Err(BrowserError::TabAsleep(_))));
        assert!(matches!(act, Err(BrowserError::TabAsleep(_))));
        assert!(matches!(navigate, Err(BrowserError::TabAsleep(_))));
        assert!(
            elapsed < Duration::from_secs(12),
            "TabAsleep must beat the 12 s deadline: {elapsed:?}"
        );
        assert!(
            protocol
                .iter()
                .all(|(method, _)| method != "Target.attachToTarget"),
            "an asleep tab must not be attached: {protocol:?}"
        );
        assert_eq!(
            BrowserError::TabAsleep(tab.id.clone()).to_string(),
            format!("tab is asleep: {}", tab.id)
        );
        browser.mark_asleep(&tab.id, false, false).await.unwrap();
        browser.snapshot(&tab.id).await.unwrap();
    }
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

/// PP-23 ② (controller half): a SIGKILLed browser is reported as disconnected, and a new
/// browser starts on the same persistent profile despite the stale endpoint file it left.
#[tokio::test]
async fn force_killed_browser_is_detected_and_its_profile_relaunches() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let config = BrowserConfig {
        profile_dir: Some(root.path().join("profile")),
        ..config
    };
    let (browser, _events) = Browser::launch(config.clone()).await.unwrap();
    assert!(browser.is_connected());
    let tab = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let pid = browser.process_id();
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while browser.is_connected() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a killed browser must be reported as disconnected");
    assert!(browser.snapshot(&tab.id).await.is_err());
    drop(browser);

    let (relaunched, _events) = Browser::launch(config).await.unwrap();
    assert!(relaunched.is_connected());
    assert_ne!(relaunched.process_id(), pid);
    let fresh = relaunched
        .create_agent_tab("owner", &server.page("welcome"))
        .await
        .unwrap();
    ready(&relaunched, &fresh).await;
    assert!(relaunched
        .snapshot(&fresh.id)
        .await
        .unwrap()
        .text
        .contains("Welcome"));
    let profile = relaunched.profile_path().to_string_lossy().into_owned();
    let pid = relaunched.process_id();
    relaunched.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

/// Chrome's `targetInfoChanged` carries the document title after a tab's first load but not after
/// a later navigation in the same tab; `tabs()` must still report the new page's title.
#[tokio::test]
async fn tabs_report_the_document_title_after_a_same_tab_navigation() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let select = server.page("select");
    browser.navigate("owner", &tab.id, &select).await.unwrap();
    ready(
        &browser,
        &openside_browser::TabInfo {
            url: select.clone(),
            ..tab.clone()
        },
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let listed = browser
            .tabs()
            .await
            .into_iter()
            .find(|entry| entry.id == tab.id)
            .unwrap();
        if listed.title == "Mock select" {
            assert_eq!(listed.url, select);
            assert_eq!(listed.opened_by, OpenedBy::Agent);
            assert_eq!(listed.owner_session_id.as_deref(), Some("owner"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tabs() kept the stale title {:?}",
            listed.title
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}
