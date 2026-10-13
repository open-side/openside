//! Headless persistent-account lifecycle and cookie isolation against temporary profiles.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(dead_code)]
mod common;
use common::{browser_config, ready, serial, Server};
use openside_browser::{
    account_spaces::{AccountSpaces, IDLE_TIMEOUT},
    Browser,
};
use parking_lot::Mutex;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[tokio::test]
async fn account_spaces_launch_idle_replace_restart_revoke_and_isolate() {
    let _serial = serial().await;
    let Some(config) = browser_config() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("browser-profiles/accounts");
    let now = Arc::new(Mutex::new(Instant::now()));
    let clock = {
        let now = now.clone();
        Arc::new(move || *now.lock())
    };
    let spaces = AccountSpaces::with_clock(clock);
    assert!(
        !root.exists(),
        "constructing a manager must not launch or create profiles"
    );
    let server = Server::start().await;
    let page = server.page("forms");
    let (main, _events) = Browser::launch(config.clone()).await.unwrap();
    let main_tab = main.create_agent_tab("main", &page).await.unwrap();
    ready(&main, &main_tab).await;
    let (a, _events) = spaces
        .open(&root, "alpha", "example.test", config.clone())
        .await
        .unwrap();
    let a_tab = a.create_agent_tab("owner", &page).await.unwrap();
    ready(&a, &a_tab).await;
    a.evaluate(
        &a_tab.id,
        "document.cookie='account=alpha;path=/;max-age=3600';true",
    )
    .await
    .unwrap();
    assert_eq!(
        main.evaluate(&main_tab.id, "document.cookie")
            .await
            .unwrap(),
        ""
    );
    assert_eq!(
        spaces.space_for_tab(&a_tab.id).await.as_deref(),
        Some("alpha")
    );
    assert!(spaces.for_tab("intruder", &a_tab.id).await.is_none());
    assert!(spaces.for_tab("owner", &a_tab.id).await.is_some());
    let (same, events) = spaces
        .open(&root, "alpha", "example.test", config.clone())
        .await
        .unwrap();
    assert_eq!(same.process_id(), a.process_id());
    assert!(events.is_none());
    *now.lock() += IDLE_TIMEOUT + Duration::from_secs(1);
    spaces.close_idle().await.unwrap();
    assert!(a.is_connected(), "an owned open tab prevents idle shutdown");
    let (b, _events) = spaces
        .open(&root, "bravo", "example.test", config.clone())
        .await
        .unwrap();
    assert!(
        a.site_cookie_keys().await.is_err(),
        "replacement must stop the first process even while callers retain Arcs"
    );
    assert!(main.is_connected());
    assert!(root.join("alpha").is_dir());
    *now.lock() += IDLE_TIMEOUT + Duration::from_secs(1);
    spaces.close_idle().await.unwrap();
    assert!(
        b.site_cookie_keys().await.is_err(),
        "unused account profile must close at the injected deadline"
    );
    spaces.close_all().await.unwrap();
    drop(spaces);
    let restarted = AccountSpaces::default();
    assert!(
        restarted.tabs("owner").await.is_empty(),
        "restart never reopens profiles"
    );
    assert!(root.join("alpha").is_dir());
    let (a_again, _events) = restarted
        .open(&root, "alpha", "example.test", config)
        .await
        .unwrap();
    let tab = a_again.create_agent_tab("owner", &page).await.unwrap();
    ready(&a_again, &tab).await;
    assert_eq!(
        a_again.evaluate(&tab.id, "document.cookie").await.unwrap(),
        "account=alpha"
    );
    restarted.revoke(&root, "alpha").await.unwrap();
    assert!(!root.join("alpha").exists());
    assert!(a_again.site_cookie_keys().await.is_err());
    assert!(root.join("bravo").exists());
    assert!(AccountSpaces::profile(&root, "../main").is_err());
    restarted.revoke(&root, "bravo").await.unwrap();
    main.shutdown().await.unwrap();
}
