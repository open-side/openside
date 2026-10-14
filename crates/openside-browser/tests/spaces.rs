//! Isolated spaces (CDP browser contexts in the managed browser) against real Chromium with
//! loopback fixtures and temporary profiles only: storage isolation from the default context
//! and between spaces, disposal wiping storage, ownership and navigation policy inside a space,
//! popups staying in their opener's space, and reconnect keeping spaces while a browser
//! restart loses them.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[allow(dead_code)]
mod common;

use std::sync::Arc;

use openside_browser::{Browser, BrowserConfig, BrowserError, BrowserEvent, OpenedBy, TabInfo};
use serde_json::{json, Value};

use common::{browser_config, event, gone, ready, serial, Server};

/// Kills a detached browser's process group if the test fails before shutting it down.
struct Kill(Option<u32>);

impl Drop for Kill {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
}

async fn storage(browser: &Browser, tab: &TabInfo) -> Value {
    browser
        .evaluate(&tab.id, "[document.cookie, localStorage.getItem('space')]")
        .await
        .unwrap()
}

async fn remember(browser: &Browser, tab: &TabInfo, value: &str) {
    browser
        .evaluate(
            &tab.id,
            &format!(
                "document.cookie = 'space={value}; path=/'; localStorage.setItem('space', '{value}'); true"
            ),
        )
        .await
        .unwrap();
}

/// A cookie or localStorage entry written in one space is visible to that space only: not to
/// the managed profile's default context and not to another session's space. Disposing a
/// space closes its tabs and wipes it; the owner's next space starts empty.
#[tokio::test]
async fn spaces_isolate_storage_and_disposal_wipes_it() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    let page = server.page("forms");

    let shared = browser.create_agent_tab("shared", &page).await.unwrap();
    ready(&browser, &shared).await;
    assert_eq!(shared.browser_context_id, None);
    remember(&browser, &shared, "shared").await;

    let first = browser.open_space("alpha").await.unwrap();
    assert_eq!(
        browser.open_space("alpha").await.unwrap(),
        first,
        "one space per owner"
    );
    let second = browser.open_space("bravo").await.unwrap();
    assert_ne!(first, second);
    let alpha = browser.create_agent_tab("alpha", &page).await.unwrap();
    ready(&browser, &alpha).await;
    let bravo = browser.create_agent_tab("bravo", &page).await.unwrap();
    ready(&browser, &bravo).await;
    assert_eq!(alpha.browser_context_id.as_deref(), Some(first.as_str()));
    assert_eq!(bravo.browser_context_id.as_deref(), Some(second.as_str()));

    // The managed profile's logins do not carry into a space.
    assert_eq!(storage(&browser, &alpha).await, json!(["", null]));
    assert_eq!(storage(&browser, &bravo).await, json!(["", null]));
    remember(&browser, &alpha, "alpha").await;
    assert_eq!(
        storage(&browser, &alpha).await,
        json!(["space=alpha", "alpha"])
    );
    assert_eq!(
        storage(&browser, &shared).await,
        json!(["space=shared", "shared"])
    );
    assert_eq!(storage(&browser, &bravo).await, json!(["", null]));

    // A second tab of the same owner shares its space.
    let again = browser.create_agent_tab("alpha", &page).await.unwrap();
    ready(&browser, &again).await;
    assert_eq!(
        storage(&browser, &again).await,
        json!(["space=alpha", "alpha"])
    );

    // Ownership stays per session inside a space, and no person adopts a space tab.
    assert!(matches!(
        browser.close_agent_tab("bravo", &alpha.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser.adopt_user_tab(&alpha.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));

    assert!(browser.close_space("alpha").await.unwrap());
    assert!(!browser.close_space("alpha").await.unwrap());
    for id in [&alpha.id, &again.id] {
        event(
            &mut events,
            |event| matches!(event, BrowserEvent::TabClosed { tab, .. } if &tab.id == id),
        )
        .await;
    }
    assert!(browser
        .tabs()
        .await
        .iter()
        .all(|tab| tab.id != alpha.id && tab.id != again.id));
    let renewed = browser.open_space("alpha").await.unwrap();
    assert_ne!(renewed, first);
    let fresh = browser.create_agent_tab("alpha", &page).await.unwrap();
    ready(&browser, &fresh).await;
    assert_eq!(storage(&browser, &fresh).await, json!(["", null]));
    // The other space and the default context are untouched by the disposal.
    assert_eq!(
        storage(&browser, &shared).await,
        json!(["space=shared", "shared"])
    );

    assert!(browser.close_space("alpha").await.unwrap());
    assert!(browser.close_space("bravo").await.unwrap());
    assert_eq!(browser.prune_spaces().await.unwrap(), 0, "no context left");
    assert!(browser.spaces().await.is_empty());
    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

/// A popup of a space tab opens in the same space and belongs to the opener's session; the
/// owner navigation policy and node-bound text insertion work in a space as in the default
/// context.
#[tokio::test]
async fn space_tabs_keep_popups_policy_and_input() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, mut events) = Browser::launch(config).await.unwrap();
    browser.set_navigation_policy(Arc::new(|_: &str, url: &str| !url.ends_with("/welcome")));
    let context = browser.open_space("alpha").await.unwrap();
    let tab = browser
        .create_agent_tab("alpha", &server.page("popup"))
        .await
        .unwrap();
    ready(&browser, &tab).await;
    remember(&browser, &tab, "alpha").await;
    browser
        .evaluate(&tab.id, "document.getElementById('popup').click(); true")
        .await
        .unwrap();
    let BrowserEvent::PopupOpened { tab: popup, .. } = event(
        &mut events,
        |event| matches!(event, BrowserEvent::PopupOpened { opener_id, .. } if opener_id == &tab.id),
    )
    .await
    else {
        unreachable!()
    };
    assert_eq!(popup.browser_context_id.as_deref(), Some(context.as_str()));
    assert_eq!(popup.owner_session_id.as_deref(), Some("alpha"));
    assert_eq!(popup.opened_by, OpenedBy::External);
    let popup = TabInfo {
        url: server.page("forms"),
        ..popup
    };
    ready(&browser, &popup).await;
    assert_eq!(
        storage(&browser, &popup).await,
        json!(["space=alpha", "alpha"])
    );
    browser.check_owned("alpha", &popup.id).await.unwrap();
    assert!(matches!(
        browser
            .navigate("bravo", &popup.id, &server.page("forms"))
            .await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    browser
        .navigate("alpha", &popup.id, &server.page("forms"))
        .await
        .unwrap();
    let popup_snapshot = browser.snapshot(&popup.id).await.unwrap();
    let field = popup_snapshot
        .nodes
        .iter()
        .find(|node| node.role == "textbox" && node.name == "Email")
        .and_then(|node| node.reference.as_deref())
        .unwrap();
    let node = browser
        .resolve_ref("alpha", &popup.id, field)
        .await
        .unwrap();
    browser
        .insert_text_node("alpha", &popup.id, &node, "popup-owner")
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&popup.id, "document.getElementById('email').value")
            .await
            .unwrap(),
        json!("popup-owner")
    );
    assert!(matches!(
        browser
            .insert_text_node("bravo", &popup.id, &node, "wrong-owner")
            .await,
        Err(BrowserError::TextInsertionFailed)
    ));
    assert_eq!(
        browser
            .evaluate(&popup.id, "document.getElementById('email').value")
            .await
            .unwrap(),
        json!("popup-owner")
    );

    // The browser-level request interception covers the space's documents too.
    assert!(browser
        .navigate("alpha", &tab.id, &server.page("welcome"))
        .await
        .is_err());
    assert_eq!(
        browser.evaluate(&tab.id, "location.href").await.unwrap(),
        json!(server.page("popup"))
    );

    let form = browser
        .create_agent_tab("alpha", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &form).await;
    let snapshot = browser.snapshot(&form.id).await.unwrap();
    let field = snapshot
        .nodes
        .iter()
        .find(|node| node.role == "textbox" && node.name == "Email")
        .and_then(|node| node.reference.clone())
        .expect("form fixture has an Email textbox");
    let node = browser
        .resolve_ref("alpha", &form.id, &field)
        .await
        .unwrap();
    browser
        .insert_text_node("alpha", &form.id, &node, "typed-in-space")
        .await
        .unwrap();
    assert_eq!(
        browser
            .evaluate(&form.id, "document.getElementById('email').value")
            .await
            .unwrap(),
        json!("typed-in-space")
    );

    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

/// A controller reconnect keeps a space (its storage, tabs and owner come back through
/// `restore_space` + `adopt_tab`); the space's tab is refused to any other owner. An unbound
/// context is pruned. After the browser restarts, the saved space no longer exists.
#[tokio::test]
async fn reconnect_restores_spaces_and_a_restart_loses_them() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let config = BrowserConfig {
        profile_dir: Some(root.path().join("profile")),
        detached: true,
        ..config
    };
    let (browser, _events) = Browser::launch(config.clone()).await.unwrap();
    let mut kill = Kill(Some(browser.process_id()));
    let shared = browser
        .create_agent_tab("shared", &server.page("forms"))
        .await
        .unwrap();
    let context = browser.open_space("alpha").await.unwrap();
    let orphan = browser.open_space("orphan").await.unwrap();
    let tab = browser
        .create_agent_tab("alpha", &server.page("forms"))
        .await
        .unwrap();
    ready(&browser, &tab).await;
    remember(&browser, &tab, "alpha").await;
    let downloads = browser.space_downloads(&context);
    let download = downloads.join("fixture");
    std::fs::write(&download, "private-download").unwrap();
    let endpoint = browser.endpoint();
    browser.detach().await.unwrap();

    let (browser, _events) = Browser::connect(endpoint).await.unwrap();
    browser.adopt_tab(&shared.id, "shared").await.unwrap();
    browser.check_owned("shared", &shared.id).await.unwrap();
    browser
        .navigate("shared", &shared.id, &server.page("forms"))
        .await
        .unwrap();
    assert!(matches!(
        browser.check_owned("alpha", &shared.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(
        download.exists(),
        "reconnecting must not wipe a surviving space's downloads"
    );
    assert!(browser.spaces().await.is_empty());
    assert!(matches!(
        browser.check_owned("alpha", &tab.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(
        matches!(
            browser.adopt_tab(&tab.id, "alpha").await,
            Err(BrowserError::OwnershipDenied(_))
        ),
        "a space tab is not adopted before its space is restored"
    );
    assert!(!browser
        .restore_space("alpha", "no-such-context")
        .await
        .unwrap());
    assert!(browser.restore_space("alpha", &context).await.unwrap());
    assert!(
        !browser.restore_space("bravo", &context).await.unwrap(),
        "a context binds to one owner"
    );
    assert!(matches!(
        browser.adopt_tab(&tab.id, "bravo").await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    browser.adopt_tab(&tab.id, "alpha").await.unwrap();
    assert_eq!(
        storage(&browser, &tab).await,
        json!(["space=alpha", "alpha"])
    );
    assert_eq!(browser.prune_spaces().await.unwrap(), 1);
    assert!(
        download.exists(),
        "pruning must preserve restored space downloads"
    );
    assert!(!browser.restore_space("orphan", &orphan).await.unwrap());
    let next = browser
        .create_agent_tab("alpha", &server.page("forms"))
        .await
        .unwrap();
    assert_eq!(next.browser_context_id.as_deref(), Some(context.as_str()));

    let profile = browser.profile_path().to_string_lossy().into_owned();
    let pid = browser.process_id();
    browser.shutdown().await.unwrap();
    kill.0 = None;
    gone(&profile, pid).await;

    let (restarted, _events) = Browser::launch(BrowserConfig {
        detached: false,
        ..config
    })
    .await
    .unwrap();
    assert!(!restarted.restore_space("alpha", &context).await.unwrap());
    let profile = restarted.profile_path().to_string_lossy().into_owned();
    let pid = restarted.process_id();
    restarted.shutdown().await.unwrap();
    gone(&profile, pid).await;
}

/// One raw CDP command from a second DevTools client, as an unrelated actor would send it.
async fn raw(browser: &Browser, method: &str, params: Value) -> Value {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let endpoint = tokio::fs::read_to_string(browser.profile_path().join("DevToolsActivePort"))
        .await
        .unwrap();
    let mut lines = endpoint.lines();
    let url = format!(
        "ws://127.0.0.1:{}{}",
        lines.next().unwrap(),
        lines.next().unwrap()
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    socket
        .send(Message::Text(
            json!({"id":1,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    loop {
        if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
            let reply: Value = serde_json::from_str(&text).unwrap();
            if reply["id"] == 1 {
                return reply["result"].clone();
            }
        }
    }
}

async fn until<T>(mut probe: impl AsyncFnMut() -> Option<T>, what: &str) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(value) = probe().await {
                return value;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"))
}

/// A closed space never falls back to the shared profile: a cancelled close keeps the context
/// pending instead of forgetting it, the owner's tabs are refused meanwhile, and
/// [`Browser::settle_spaces`] finishes the disposal. Reopening gives an empty space.
#[tokio::test]
async fn closed_spaces_fail_closed_and_a_cancelled_disposal_is_retried() {
    use futures_util::FutureExt;
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let page = server.page("forms");
    browser.open_space("alpha").await.unwrap();
    let alpha = browser.create_agent_tab("alpha", &page).await.unwrap();
    ready(&browser, &alpha).await;
    remember(&browser, &alpha, "alpha").await;

    // Cancelled after its first poll: the CDP disposal is in flight or not yet sent.
    assert!(Box::pin(browser.close_space("alpha"))
        .now_or_never()
        .is_none());
    assert_eq!(browser.space("alpha").await, None);
    assert_eq!(browser.closing_spaces().await, 1, "the obligation is kept");
    assert!(
        browser.tabs().await.iter().any(|tab| tab.id == alpha.id),
        "cancellation happens before disposal, retaining the old tab"
    );
    assert!(matches!(
        browser.check_owned("alpha", &alpha.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser.navigate("alpha", &alpha.id, &page).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    assert!(matches!(
        browser.thumbnail("alpha", &alpha.id).await,
        Err(BrowserError::OwnershipDenied(_))
    ));
    let replacement = browser.open_space("alpha").await.unwrap();
    assert!(
        matches!(
            browser.check_owned("alpha", &alpha.id).await,
            Err(BrowserError::OwnershipDenied(_))
        ),
        "reopening never authorizes the previous context's tab"
    );
    browser.close_space("alpha").await.unwrap();
    assert!(!browser.space_downloads(&replacement).exists());
    let before = browser.tabs().await.len();
    assert!(matches!(
        browser.create_agent_tab("alpha", &page).await,
        Err(BrowserError::SpaceClosed(_))
    ));
    assert!(
        browser.tabs().await.len() <= before,
        "no tab in the shared profile"
    );
    browser.settle_spaces().await.unwrap();
    assert_eq!(browser.closing_spaces().await, 0);
    let listed = raw(&browser, "Target.getBrowserContexts", json!({})).await;
    assert_eq!(listed["browserContextIds"], json!([]), "{listed}");
    until(
        async || {
            browser
                .tabs()
                .await
                .iter()
                .all(|tab| tab.id != alpha.id)
                .then_some(())
        },
        "the space tab closes",
    )
    .await;

    let again = browser.open_space("alpha").await.unwrap();
    let tab = browser.create_agent_tab("alpha", &page).await.unwrap();
    ready(&browser, &tab).await;
    assert_eq!(tab.browser_context_id.as_deref(), Some(again.as_str()));
    assert_eq!(storage(&browser, &tab).await, json!(["", null]));
    browser.shutdown().await.unwrap();
}

/// Downloads of a space go to its own 0700 directory, never the shared profile's, are kept
/// apart between spaces with the same file name, and are deleted with the space; a leftover
/// directory of no bound space is deleted by the prune.
#[tokio::test]
async fn space_downloads_are_private_and_wiped_with_the_space() {
    use std::os::unix::fs::PermissionsExt;
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let mut files = Vec::new();
    for owner in ["alpha", "bravo"] {
        let context = browser.open_space(owner).await.unwrap();
        let tab = browser
            .create_agent_tab(owner, &server.page("download"))
            .await
            .unwrap();
        ready(&browser, &tab).await;
        browser
            .evaluate(&tab.id, "document.getElementById('download').click(); true")
            .await
            .unwrap();
        let directory = browser.space_downloads(&context);
        let file = until(
            async || {
                let mut entries = std::fs::read_dir(&directory).ok()?;
                entries.find_map(|entry| {
                    let path = entry.ok()?.path();
                    (std::fs::read_to_string(&path).ok()? == "openside local download fixture\n")
                        .then_some(path)
                })
            },
            "the space download lands in its directory",
        )
        .await;
        assert_eq!(
            std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        files.push((directory, file));
    }
    let shared = browser.profile_path().join("downloads");
    assert!(
        std::fs::read_dir(&shared).map_or(true, |mut entries| entries.next().is_none()),
        "nothing in the shared download directory"
    );
    assert_ne!(files[0].0, files[1].0);

    assert!(browser.close_space("alpha").await.unwrap());
    assert!(!files[0].0.exists(), "the space's downloads are wiped");
    assert!(files[1].1.exists(), "another space keeps its own");
    let leftover = browser
        .profile_path()
        .join("space-downloads")
        .join("leftover");
    std::fs::create_dir_all(&leftover).unwrap();
    assert_eq!(browser.prune_spaces().await.unwrap(), 0);
    assert!(
        !leftover.exists(),
        "a directory of no bound space is deleted"
    );
    assert!(files[1].1.exists());
    browser.shutdown().await.unwrap();
    assert!(
        !files[1].0.exists(),
        "clean shutdown wipes private downloads"
    );
}

/// A page created inside a space by anything but the controller, with no opener, still
/// belongs to the space's owner: it is never an ownerless tab outside navigation policy.
#[tokio::test]
async fn targets_in_a_space_belong_to_its_owner_without_an_opener() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let server = Server::start().await;
    let (browser, _events) = Browser::launch(config).await.unwrap();
    let context = browser.open_space("alpha").await.unwrap();
    let created = raw(
        &browser,
        "Target.createTarget",
        json!({"url":server.page("forms"),"browserContextId":context,"newWindow":true,"background":true}),
    )
    .await;
    let id = created["targetId"].as_str().unwrap().to_owned();
    let tab = until(
        async || browser.tabs().await.into_iter().find(|tab| tab.id == id),
        "the foreign target reaches the ledger",
    )
    .await;
    assert_eq!(tab.owner_session_id.as_deref(), Some("alpha"));
    assert_eq!(tab.browser_context_id.as_deref(), Some(context.as_str()));
    assert_eq!(tab.opened_by, OpenedBy::External);
    browser.shutdown().await.unwrap();
}

#[tokio::test]
async fn fresh_launch_and_verified_death_sweep_private_downloads() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let profile = root.path().join("profile");
    let stale = profile.join("space-downloads/previous-process");
    std::fs::create_dir_all(&stale).unwrap();
    std::fs::write(stale.join("fixture"), "private-download").unwrap();
    let config = BrowserConfig {
        profile_dir: Some(profile.clone()),
        ..config
    };
    let (browser, _events) = Browser::launch(config.clone()).await.unwrap();
    assert!(
        !stale.exists(),
        "fresh launch sweeps persisted orphan downloads"
    );
    let download_root = profile.join("space-downloads");
    std::fs::remove_dir(&download_root).unwrap();
    std::fs::write(&download_root, "temporarily-unreadable-directory").unwrap();
    assert!(matches!(
        browser.prune_spaces().await,
        Err(BrowserError::Io(_))
    ));
    std::fs::remove_file(&download_root).unwrap();
    std::fs::create_dir_all(&stale).unwrap();
    browser.settle_spaces().await.unwrap();
    assert!(!stale.exists(), "a failed prune remains due for retry");
    let context = browser.open_space("alpha").await.unwrap();
    let downloads = browser.space_downloads(&context);
    std::fs::write(downloads.join("fixture"), "private-download").unwrap();
    let endpoint = browser.endpoint();
    nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(i32::try_from(browser.process_id()).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    until(
        async || (!browser.is_connected()).then_some(()),
        "the killed browser disconnects",
    )
    .await;
    browser.settle_spaces().await.unwrap();
    assert!(
        !downloads.exists(),
        "periodic cleanup wipes downloads only after verifying browser death"
    );
    drop(browser);
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(downloads.join("fixture"), "left-by-crash").unwrap();
    assert!(Browser::connect(endpoint).await.is_err());
    assert!(
        !downloads.exists(),
        "dead-endpoint recovery sweeps private downloads"
    );
    std::fs::create_dir_all(&downloads).unwrap();
    let (browser, _events) = Browser::launch(config).await.unwrap();
    assert!(!downloads.exists());
    let context = browser.open_space("alpha").await.unwrap();
    let downloads = browser.space_downloads(&context);
    std::fs::write(downloads.join("fixture"), "private-download").unwrap();
    browser.terminate().await.unwrap();
    assert!(
        !downloads.exists(),
        "explicit verified termination also sweeps downloads"
    );
    assert!(matches!(
        browser.shutdown().await,
        Err(BrowserError::ProcessExit(status))
            if std::os::unix::process::ExitStatusExt::signal(&status)
                == Some(nix::sys::signal::Signal::SIGKILL as i32)
    ));
}
