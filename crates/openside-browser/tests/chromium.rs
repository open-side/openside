//! Real Chromium integration coverage using only loopback fixtures and temporary profiles.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

use openside_browser::{
    Action, Browser, BrowserConfig, BrowserEndpoint, BrowserError, BrowserEvent, DownloadStatus,
    NodeHandle, OpenedBy, Snapshot,
};
use serde_json::json;

use common::{browser_config, event, gone, ready, serial, Server, FIXTURES};

fn reference(snapshot: &Snapshot, role: &str, name: &str) -> String {
    snapshot
        .nodes
        .iter()
        .find(|node| node.role == role && node.name == name)
        .unwrap_or_else(|| panic!("missing {role} {name} in snapshot"))
        .reference
        .clone()
        .expect("interactive node must have a reference")
}

#[tokio::test]
async fn launch_connect_shutdown_and_drop_remove_processes_and_profiles() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    for graceful in [true, false] {
        let (browser, _events) = Browser::launch(config.clone()).await.unwrap();
        let profile = browser.profile_path().to_string_lossy().into_owned();
        let pid = browser.process_id();
        assert!(std::path::Path::new(&profile).exists());
        if graceful {
            browser.shutdown().await.unwrap();
        } else {
            drop(browser);
        }
        gone(&profile, pid).await;
        assert!(!std::path::Path::new(&profile).exists());
    }
    let missing = BrowserConfig::new(tempfile::tempdir().unwrap().path().join("missing-chromium"));
    match Browser::launch(missing).await {
        Err(error) => assert!(error.to_string().contains("install Chromium")),
        Ok(_) => panic!("missing Chromium must fail, not skip"),
    }
}

#[tokio::test]
async fn background_viewport_ledger_sleep_gate_warning_and_screenshot() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    let initial = event(&mut events, |event| matches!(event, BrowserEvent::TabOpened { tab } if tab.opened_by == OpenedBy::External)).await;
    let BrowserEvent::TabOpened { tab: external } = initial else {
        unreachable!()
    };
    assert!(matches!(
        browser.close_agent_tab("owner", &external.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    browser.adopt_user_tab(&external.id).await.unwrap();
    assert!(browser
        .tabs()
        .await
        .iter()
        .any(|tab| tab.id == external.id && tab.opened_by == OpenedBy::User));
    assert!(matches!(
        browser.close_agent_tab("owner", &external.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    let mut tabs = Vec::new();
    for _ in 0..4 {
        tabs.push(
            browser
                .create_agent_tab("owner", &server.page("forms"))
                .await
                .unwrap(),
        );
    }
    event(&mut events, |event| {
        matches!(event, BrowserEvent::TooManyTabs { count: 5 })
    })
    .await;
    let mut windows = std::collections::HashSet::new();
    windows.insert(common::window_id(browser.profile_path(), &external.id).await);
    for tab in &tabs {
        assert!(
            windows.insert(common::window_id(browser.profile_path(), &tab.id).await),
            "agent tabs must use separate windows"
        );
    }
    let tab = &tabs[0];
    ready(&browser, tab).await;
    assert_eq!(
        browser
            .evaluate(&tab.id, "[window.innerWidth,window.innerHeight]")
            .await
            .unwrap(),
        json!([1440, 900])
    );
    assert!(matches!(
        browser.close_agent_tab("different-owner", &tab.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser.adopt_user_tab(&tab.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    let before = browser
        .evaluate(&tab.id, "document.hasFocus()")
        .await
        .unwrap();
    assert_eq!(
        before,
        json!(false),
        "background agent windows must not have focus"
    );
    let image = browser.screenshot(&tab.id).await.unwrap();
    assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.hasFocus()")
            .await
            .unwrap(),
        before
    );
    for (discarded, unloaded) in [(true, false), (false, true)] {
        browser
            .mark_asleep(&tab.id, discarded, unloaded)
            .await
            .unwrap();
        assert!(matches!(
            browser.attach(&tab.id).await,
            Err(BrowserError::TabAsleep(_))
        ));
        browser.mark_asleep(&tab.id, false, false).await.unwrap();
        browser.attach(&tab.id).await.unwrap();
    }
    assert!(matches!(
        browser.attach("non-existent-target").await,
        Err(BrowserError::TabNotReady(_))
    ));
    browser.close_agent_tab("owner", &tab.id).await.unwrap();
    event(&mut events, |event| matches!(event, BrowserEvent::TabClosed { tab: closed } if closed.id == tab.id && closed.opened_by == OpenedBy::Agent && closed.owner_session_id.as_deref() == Some("owner"))).await;
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

#[tokio::test]
async fn whole_attach_operation_races_configurable_timeout() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let config = BrowserConfig {
        attach_timeout: Duration::ZERO,
        ..config
    };
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    let opened = event(&mut events, |event| {
        matches!(event, BrowserEvent::TabOpened { .. })
    })
    .await;
    let BrowserEvent::TabOpened { tab } = opened else {
        unreachable!()
    };
    assert!(matches!(
        browser.attach(&tab.id).await,
        Err(BrowserError::Timeout("tab attach"))
    ));
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

#[tokio::test]
async fn accessibility_refs_survive_dom_changes_and_diff_is_complete() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("snapshot-owner", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &tab).await;
    let before = browser.snapshot(&tab.id).await.unwrap();
    let save = before
        .nodes
        .iter()
        .find(|node| node.role == "button" && node.name == "Save")
        .unwrap()
        .reference
        .clone()
        .unwrap();
    let removed = before
        .nodes
        .iter()
        .find(|node| node.role == "button" && node.name == "Remove")
        .unwrap()
        .reference
        .clone()
        .unwrap();
    assert!(!before.text.contains("CANARY-form-secret"));
    assert!(before
        .nodes
        .iter()
        .all(|node| node.role != "none" && !(node.role == "generic" && node.name.is_empty())));
    browser.evaluate(&tab.id, "document.getElementById('save').textContent='Saved';document.getElementById('remove').remove();const b=document.createElement('button');b.textContent='Added action';document.body.append(b);true").await.unwrap();
    let after = browser.snapshot(&tab.id).await.unwrap();
    assert_eq!(
        after
            .nodes
            .iter()
            .find(|node| node.role == "button" && node.name == "Saved")
            .unwrap()
            .reference
            .as_ref(),
        Some(&save)
    );
    let diff = before.diff(&after);
    assert!(diff.changed.contains(&save));
    assert!(diff.removed.contains(&removed));
    assert!(!diff.added.is_empty());
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn popup_download_dialog_and_tab_closed_events() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    let popup = browser
        .create_agent_tab("events-owner", &server.page("popup"))
        .await
        .unwrap();
    ready(&browser, &popup).await;
    browser
        .evaluate(&popup.id, "document.getElementById('popup').click();true")
        .await
        .unwrap();
    let opened = event(&mut events, |event| matches!(event, BrowserEvent::PopupOpened { opener_id, .. } if opener_id == &popup.id)).await;
    let BrowserEvent::PopupOpened { tab: popup_tab, .. } = opened else {
        unreachable!()
    };
    assert_eq!(popup_tab.opened_by, OpenedBy::External);
    assert_eq!(popup_tab.owner_session_id.as_deref(), Some("events-owner"));
    assert!(matches!(
        browser.close_agent_tab("events-owner", &popup_tab.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    browser
        .evaluate(&popup.id, "window.mockPopup.close();true")
        .await
        .unwrap();
    event(&mut events, |event| matches!(event, BrowserEvent::TabClosed { tab } if tab.id == popup_tab.id && tab.opened_by == OpenedBy::External)).await;
    let download = browser
        .create_agent_tab("events-owner", &server.page("download"))
        .await
        .unwrap();
    ready(&browser, &download).await;
    browser
        .evaluate(
            &download.id,
            "document.getElementById('download').click();true",
        )
        .await
        .unwrap();
    let started = event(&mut events, |event| {
        matches!(event, BrowserEvent::DownloadStarted { .. })
    })
    .await;
    let BrowserEvent::DownloadStarted {
        guid,
        suggested_filename,
        tab_id,
        url,
    } = started
    else {
        unreachable!()
    };
    assert_eq!(tab_id.as_deref(), Some(download.id.as_str()));
    assert_eq!(url, server.page("download-file"));
    assert_eq!(suggested_filename, "fixture.txt");
    event(&mut events, |event| matches!(event, BrowserEvent::DownloadFinished { guid: finished, status: DownloadStatus::Completed } if finished == &guid)).await;
    assert_eq!(
        tokio::fs::read(browser.profile_path().join("downloads").join(&guid))
            .await
            .unwrap(),
        b"openside local download fixture\n"
    );
    let dialog = browser
        .create_agent_tab("events-owner", &server.page("dialog"))
        .await
        .unwrap();
    ready(&browser, &dialog).await;
    let (triggered, ()) = tokio::join!(
        browser.evaluate(&dialog.id, "document.getElementById('dialog').click();true"),
        async {
            event(&mut events, |event| matches!(event, BrowserEvent::DialogOpened { tab_id, kind, message } if tab_id == &dialog.id && kind == "alert" && message == "Local fixture dialog")).await;
            browser.handle_dialog(&dialog.id, true, None).await.unwrap();
        }
    );
    triggered.unwrap();
    browser
        .close_agent_tab("events-owner", &dialog.id)
        .await
        .unwrap();
    event(
        &mut events,
        |event| matches!(event, BrowserEvent::TabClosed { tab } if tab.id == dialog.id),
    )
    .await;
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

#[tokio::test]
async fn snapshot_size_measurements() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    println!("| fixture | raw AX JSON bytes | compact text bytes | compact/raw | ");
    println!("|---|---:|---:|---:|");
    for fixture in FIXTURES {
        let tab = browser
            .create_agent_tab("bench-owner", &server.page(fixture))
            .await
            .unwrap();
        ready(&browser, &tab).await;
        let snapshot = browser.snapshot(&tab.id).await.unwrap();
        let bytes = snapshot.text.len();
        println!(
            "| {fixture}.html | {} | {bytes} | {:.4} |",
            snapshot.raw_bytes,
            bytes as f64 / snapshot.raw_bytes as f64
        );
        assert!(bytes > 0 && bytes < snapshot.raw_bytes);
        if *fixture == "long-list" {
            assert_eq!(
                snapshot
                    .nodes
                    .iter()
                    .filter(|node| node.role == "button")
                    .count(),
                300
            );
            assert!(snapshot.text.contains("Open item 300"));
        }
        if *fixture == "nested-divs" {
            assert_eq!(
                snapshot
                    .nodes
                    .iter()
                    .filter(|node| node.role == "button")
                    .count(),
                60
            );
            assert!(snapshot
                .nodes
                .iter()
                .filter(|node| node.role == "button")
                .all(|node| node.depth < 5));
        }
        browser
            .close_agent_tab("bench-owner", &tab.id)
            .await
            .unwrap();
    }
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

#[tokio::test]
async fn click_navigation_hover_focus_origin_and_stale_references() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("login"))
        .await
        .unwrap();
    assert_eq!(browser.origin(&tab.id).await.unwrap(), server.url);
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let username = reference(&snapshot, "textbox", "Username");
    let button = reference(&snapshot, "button", "Log in");
    let focus = browser
        .act("owner", &tab.id, &username, Action::Focus)
        .await
        .unwrap();
    assert!(!focus.navigated);
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.activeElement.id")
            .await
            .unwrap(),
        "username"
    );
    let hover = browser
        .act("owner", &tab.id, &button, Action::Hover)
        .await
        .unwrap();
    assert!(!hover.navigated);
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.getElementById('submit').dataset.hovered")
            .await
            .unwrap(),
        "true"
    );
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.activeElement.id")
            .await
            .unwrap(),
        "username"
    );
    assert!(matches!(
        browser
            .act("other-owner", &tab.id, &button, Action::Click)
            .await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser
            .navigate("other-owner", &tab.id, &server.page("welcome"))
            .await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser
            .act("owner", &tab.id, "e999999", Action::Focus)
            .await,
        Err(BrowserError::StaleReference)
    ));
    let outcome = browser
        .act("owner", &tab.id, &button, Action::Click)
        .await
        .unwrap();
    assert!(outcome.navigated);
    assert_eq!(outcome.url, server.page("welcome"));
    let stale = browser
        .act("owner", &tab.id, &username, Action::Focus)
        .await
        .unwrap_err();
    assert!(matches!(stale, BrowserError::StaleReference));
    assert!(stale.to_string().contains("new snapshot"));
    browser
        .navigate("owner", &tab.id, &server.page("login"))
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&tab.id, "[document.readyState, location.href]")
            .await
            .unwrap(),
        json!(["complete", server.page("login")])
    );
    let fresh = browser.snapshot(&tab.id).await.unwrap();
    assert_ne!(reference(&fresh, "textbox", "Username"), username);
    assert!(matches!(
        browser
            .act("owner", &tab.id, &username, Action::Focus)
            .await,
        Err(BrowserError::StaleReference)
    ));
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn type_inserts_text_and_submit_navigates() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("login"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let username = reference(&snapshot, "textbox", "Username");
    let password = reference(&snapshot, "textbox", "Password");
    let typed = browser
        .act(
            "owner",
            &tab.id,
            &username,
            Action::Type {
                text: "local-user".to_owned(),
                submit: false,
            },
        )
        .await
        .unwrap();
    assert!(!typed.navigated);
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.getElementById('username').value")
            .await
            .unwrap(),
        "local-user"
    );
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.activeElement.id")
            .await
            .unwrap(),
        "username"
    );
    let submitted = browser
        .act(
            "owner",
            &tab.id,
            &password,
            Action::Type {
                text: "local-password".to_owned(),
                submit: true,
            },
        )
        .await
        .unwrap();
    assert!(submitted.navigated);
    assert_eq!(submitted.url, server.page("welcome"));
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn press_enter_submits_the_target_form() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("login"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let username = reference(&snapshot, "textbox", "Username");
    let outcome = browser
        .act(
            "owner",
            &tab.id,
            &username,
            Action::Press {
                key: "Enter".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(outcome.navigated);
    assert_eq!(outcome.url, server.page("welcome"));
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn select_values_emits_events_and_rejects_missing_options() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("select"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let plan = reference(&snapshot, "combobox", "Plan");
    let colors = reference(&snapshot, "listbox", "Colors");
    let outcome = browser
        .act(
            "owner",
            &tab.id,
            &plan,
            Action::Select {
                values: vec!["plus".to_owned()],
            },
        )
        .await
        .unwrap();
    assert!(!outcome.navigated);
    assert_eq!(
        browser.evaluate(&tab.id, "(() => { const s=document.getElementById('plan'); return [s.value,s.dataset.input,s.dataset.changed]; })()").await.unwrap(),
        json!(["plus", "true", "true"])
    );
    browser
        .act(
            "owner",
            &tab.id,
            &colors,
            Action::Select {
                values: vec!["red".to_owned(), "blue".to_owned()],
            },
        )
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(
                &tab.id,
                "Array.from(document.getElementById('colors').selectedOptions, o => o.value)"
            )
            .await
            .unwrap(),
        json!(["red", "blue"])
    );
    assert!(matches!(
        browser
            .act(
                "owner",
                &tab.id,
                &plan,
                Action::Select {
                    values: vec!["missing".to_owned()]
                }
            )
            .await,
        Err(BrowserError::InvalidAction(_))
    ));
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.getElementById('plan').value")
            .await
            .unwrap(),
        "plus"
    );
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn secret_insert_text_errors_never_echo_canary() {
    const CANARY: &str = "CANARY-browser-insert-7d389f";
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("login"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let password = reference(&snapshot, "textbox", "Password");
    browser
        .insert_text("owner", &tab.id, &password, CANARY)
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.getElementById('password').value")
            .await
            .unwrap(),
        CANARY
    );
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.activeElement.id")
            .await
            .unwrap(),
        "password"
    );
    for (owner, id, target) in [
        ("other-owner", tab.id.as_str(), password.as_str()),
        ("owner", CANARY, password.as_str()),
        ("owner", tab.id.as_str(), CANARY),
    ] {
        let error = browser
            .insert_text(owner, id, target, CANARY)
            .await
            .unwrap_err();
        assert!(!error.to_string().contains(CANARY));
        assert!(!format!("{error:?}").contains(CANARY));
    }
    browser
        .navigate("owner", &tab.id, &server.page("welcome"))
        .await
        .unwrap();
    let error = browser
        .insert_text("owner", &tab.id, &password, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::StaleReference));
    assert!(!error.to_string().contains(CANARY));
    assert!(!format!("{error:?}").contains(CANARY));
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn secret_insertion_is_bound_to_the_resolved_node_despite_page_focus_tricks() {
    const CANARY: &str = "CANARY-bound-node-insert-41c7";
    const HOLDERS: &str =
        "Array.from(document.querySelectorAll('input')).filter(i => i.value.includes('CANARY')).map(i => i.id)";
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("secret-race"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let target = reference(&snapshot, "textbox", "Target");
    let swapped = reference(&snapshot, "textbox", "Swapped");
    let controlled = reference(&snapshot, "textbox", "Controlled");
    let locked = reference(&snapshot, "textbox", "Locked");

    // Focus moves to the decoy and the page lies about activeElement; the value still binds.
    browser
        .insert_text("owner", &tab.id, &target, CANARY)
        .await
        .unwrap();
    assert_eq!(
        browser.evaluate(&tab.id, HOLDERS).await.unwrap(),
        json!(["target"])
    );
    assert_eq!(
        browser
            .evaluate(&tab.id, "target.value === 'CANARY-bound-node-insert-41c7'")
            .await
            .unwrap(),
        true
    );

    // The bound node is replaced by a focused lookalike: fail closed, nothing written anywhere.
    let error = browser
        .insert_text("owner", &tab.id, &swapped, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::StaleReference), "{error:?}");
    assert!(!format!("{error:?}").contains(CANARY));
    assert_eq!(
        browser.evaluate(&tab.id, HOLDERS).await.unwrap(),
        json!(["target"])
    );

    let error = browser
        .insert_text("owner", &tab.id, &locked, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::InvalidAction(_)), "{error:?}");
    assert!(!format!("{error:?}").contains(CANARY));
    assert_eq!(
        browser.evaluate(&tab.id, HOLDERS).await.unwrap(),
        json!(["target"])
    );

    browser
        .insert_text("owner", &tab.id, &controlled, CANARY)
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(
                &tab.id,
                "[window.controlledState === 'CANARY-bound-node-insert-41c7', controlled.value === window.controlledState, window.controlledChanges]"
            )
            .await
            .unwrap(),
        json!([true, true, 1])
    );
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn secret_insertion_ignores_page_world_setter_and_builtin_hooks() {
    const CANARY: &str = "CANARY-isolated-world-insert-90ad";
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("secret-hooks"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let field = reference(&snapshot, "textbox", "Field");
    browser
        .insert_text("owner", &tab.id, &field, CANARY)
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(
                &tab.id,
                "[field.value === 'CANARY-isolated-world-insert-90ad', window.__leak, window.events]"
            )
            .await
            .unwrap(),
        json!([true, [], [format!("input:{}", CANARY.len()), "change"]])
    );
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn latest_snapshot_and_connected_nodes_gate_actions() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    let external = event(&mut events, |event| matches!(event, BrowserEvent::TabOpened { tab } if tab.opened_by == OpenedBy::External)).await;
    let BrowserEvent::TabOpened { tab: external } = external else {
        unreachable!()
    };
    browser.adopt_user_tab(&external.id).await.unwrap();
    assert!(matches!(
        browser
            .act("owner", &external.id, "e1", Action::Focus)
            .await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser
            .navigate("owner", &external.id, &server.page("login"))
            .await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    let tab = browser
        .create_agent_tab("owner", &server.page("forms"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let removed = reference(&snapshot, "button", "Remove");
    browser
        .evaluate(&tab.id, "document.getElementById('remove').remove()")
        .await
        .unwrap();
    assert!(matches!(
        browser.act("owner", &tab.id, &removed, Action::Click).await,
        Err(BrowserError::StaleReference)
    ));
    browser.snapshot(&tab.id).await.unwrap();
    assert!(matches!(
        browser.act("owner", &tab.id, &removed, Action::Click).await,
        Err(BrowserError::StaleReference)
    ));
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn detached_browser_reconnect_restores_document_bound_secret_input() {
    const CANARY: &str = "CANARY-reconnected-browser-input-b8172";
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let profile = directory.path().join("persistent-profile");
    let config = BrowserConfig {
        profile_dir: Some(profile.clone()),
        detached: true,
        ..config
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("login"))
        .await
        .unwrap();
    let snapshot = browser.snapshot(&tab.id).await.unwrap();
    let password = reference(&snapshot, "textbox", "Password");
    let handle = browser
        .resolve_ref("owner", &tab.id, &password)
        .await
        .unwrap();
    assert_eq!(handle.origin, server.url);
    assert_eq!(
        handle.document_id,
        browser.document_id(&tab.id).await.unwrap()
    );
    let handle: NodeHandle = serde_json::from_value(serde_json::to_value(handle).unwrap()).unwrap();
    let endpoint: BrowserEndpoint =
        serde_json::from_value(serde_json::to_value(browser.endpoint()).unwrap()).unwrap();
    let pid = endpoint.pid;
    drop(browser);

    let (browser, _events) = Browser::connect(endpoint.clone()).await.unwrap();
    assert!(browser.tabs().await.iter().any(|live| live.id == tab.id));
    let denied = browser
        .insert_text_node("owner", &tab.id, &handle, CANARY)
        .await
        .unwrap_err();
    assert!(!denied.to_string().contains(CANARY));
    assert!(!format!("{denied:?}").contains(CANARY));
    browser.adopt_tab(&tab.id, "owner").await.unwrap();
    browser.adopt_tab(&tab.id, "owner").await.unwrap();
    assert!(matches!(
        browser.adopt_tab(&tab.id, "other-owner").await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert_eq!(
        browser.document_id(&tab.id).await.unwrap(),
        handle.document_id
    );
    browser
        .insert_text_node("owner", &tab.id, &handle, CANARY)
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&tab.id, "document.getElementById('password').value")
            .await
            .unwrap(),
        CANARY
    );
    let mut mismatched_origin = handle.clone();
    mismatched_origin.origin = format!("http://{CANARY}.invalid");
    let error = browser
        .insert_text_node("owner", &tab.id, &mismatched_origin, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::OriginMismatch));
    assert!(!error.to_string().contains(CANARY));
    assert!(!format!("{error:?}").contains(CANARY));
    let mut missing_node = handle.clone();
    missing_node.backend_node_id = u64::MAX;
    let error = browser
        .insert_text_node("owner", &tab.id, &missing_node, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::StaleReference));
    assert!(!error.to_string().contains(CANARY));
    assert!(!format!("{error:?}").contains(CANARY));
    browser
        .navigate("owner", &tab.id, &server.page("login"))
        .await
        .unwrap();
    let error = browser
        .insert_text_node("owner", &tab.id, &handle, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::StaleReference));
    assert!(!error.to_string().contains(CANARY));
    assert!(!format!("{error:?}").contains(CANARY));
    browser.detach().await.unwrap();

    let (browser, _events) = Browser::connect(endpoint.clone()).await.unwrap();
    browser.adopt_tab(&tab.id, "owner").await.unwrap();
    let error = browser
        .insert_text_node("owner", &tab.id, &handle, CANARY)
        .await
        .unwrap_err();
    assert!(matches!(error, BrowserError::StaleReference));
    browser.shutdown().await.unwrap();
    gone(&profile.to_string_lossy(), pid).await;
    assert!(
        profile.is_dir(),
        "configured persistent profiles must remain on shutdown"
    );
    assert!(matches!(
        Browser::connect(endpoint).await,
        Err(BrowserError::InvalidEndpoint(_))
    ));
}

#[tokio::test]
async fn explicit_shutdown_terminates_detached_temporary_browser() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let config = BrowserConfig {
        detached: true,
        ..config
    };
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let endpoint = browser.endpoint();
    browser.shutdown().await.unwrap();
    gone(&endpoint.profile_dir.to_string_lossy(), endpoint.pid).await;
    assert!(!endpoint.profile_dir.exists());
}

#[tokio::test]
async fn a_reattached_browser_reports_whether_it_was_started_with_the_document_isolation() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    // Started here, so isolated; the running process says so again after re-adoption.
    let (browser, _events) = Browser::launch(BrowserConfig {
        detached: true,
        ..config.clone()
    })
    .await
    .unwrap();
    assert!(browser.documents_isolated());
    let endpoint = browser.endpoint();
    drop(browser);
    let (browser, _events) = Browser::connect(endpoint.clone()).await.unwrap();
    assert!(browser.documents_isolated());
    browser.shutdown().await.unwrap();
    gone(&endpoint.profile_dir.to_string_lossy(), endpoint.pid).await;

    // Started like an older build would have started it: no isolation switch, no preferences.
    let profile = tempfile::tempdir().unwrap();
    let mut command = tokio::process::Command::new(&config.executable);
    command
        .args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "about:blank",
        ])
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    common::supervise(&mut command);
    let mut child = command.spawn().unwrap();
    let pid = child.id().unwrap();
    let active = profile.path().join("DevToolsActivePort");
    let mut saved = String::new();
    for _ in 0..200 {
        saved = std::fs::read_to_string(&active).unwrap_or_default();
        if saved.lines().count() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut lines = saved.lines();
    let (port, path) = (lines.next().unwrap(), lines.next().unwrap());
    let (older, _events) = Browser::connect(BrowserEndpoint {
        pid,
        ws_url: format!("ws://127.0.0.1:{port}{path}"),
        profile_dir: profile.path().to_owned(),
    })
    .await
    .unwrap();
    assert!(!older.documents_isolated());
    // Here the adopted browser is a child of this test, so someone must reap it for the
    // termination to be observable; a real adopted browser has init for a parent.
    let reaper = tokio::spawn(async move { child.wait().await });
    older.terminate().await.unwrap();
    reaper.await.unwrap().unwrap();
}

#[tokio::test]
async fn pointer_hit_test_describes_what_input_reaches_on_a_scrolled_page() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("scrolled"))
        .await
        .unwrap();
    // Viewport (60, 700) is document y=1500 here, where "Pay now" sits; document y=700
    // holds "Benign". A hit-test that read the point as a document coordinate would label
    // the harmless button while the click lands on the payment one.
    browser
        .evaluate(&tab.id, "window.scrollTo(0, 800)")
        .await
        .unwrap();
    let hit = browser
        .hit_test("owner", &tab.id, 60.0, 700.0)
        .await
        .unwrap();
    assert!(
        hit.labels
            .iter()
            .any(|label| label.role == "button" && label.name == "Pay now"),
        "{:?}",
        hit.labels
    );
    // The top of the viewport must also resolve: a document-coordinate read finds nothing
    // above the scroll offset.
    assert!(browser.hit_test("owner", &tab.id, 60.0, 10.0).await.is_ok());
    browser
        .pointer(
            "owner",
            &tab.id,
            openside_browser::PointerAction::Click {
                x: 60.0,
                y: 700.0,
                clicks: 1,
            },
            &hit,
        )
        .await
        .unwrap();
    assert_eq!(
        browser.evaluate(&tab.id, "document.title").await.unwrap(),
        "Pay clicked"
    );
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn navigation_to_a_server_that_never_answers_fails_without_wedging_the_tab() {
    let _serial = serial().await;
    let Some(mut config) = browser_config() else {
        return;
    };
    config.command_timeout = Duration::from_secs(2);
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let tab = browser
        .create_agent_tab("owner", &server.page("welcome"))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let failed = browser
        .navigate("owner", &tab.id, &server.page("never"))
        .await;
    assert!(
        matches!(failed, Err(BrowserError::Timeout(_))),
        "{failed:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    // The pending load was cancelled: the tab answers at once and navigates elsewhere.
    tokio::time::timeout(
        Duration::from_secs(8),
        browser.navigate("owner", &tab.id, &server.page("login")),
    )
    .await
    .expect("the tab stayed busy with the abandoned navigation")
    .unwrap();
    assert_eq!(browser.url(&tab.id).await.unwrap(), server.page("login"));
    browser.shutdown().await.unwrap();
}
