//! Linux core/browser operation is independent of the Wayland/X11 desktop session.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(dead_code)]
mod common;

use openside_browser::Browser;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn headless_controller_works_in_wayland_and_x11_session_environments() {
    let Some(mut config) = common::browser_config() else {
        return;
    };
    let server = common::Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let executable = config.executable.clone();
    let quote =
        |path: &std::path::Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    for session in ["wayland", "x11"] {
        let wrapper = root.path().join(format!("browser-{session}"));
        // Invalid display endpoints prove this test cannot attach to the real desktop.
        std::fs::write(&wrapper, format!("#!/bin/sh\nexport XDG_SESSION_TYPE={session}\nexport DISPLAY=invalid-x11\nexport WAYLAND_DISPLAY=invalid-wayland\nexec {} \"$@\"\n", quote(&executable))).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        config.executable = wrapper;
        let (browser, _events) = Browser::launch(config.clone()).await.unwrap();
        let tab = browser
            .create_agent_tab("platform-test", &server.page("forms"))
            .await
            .unwrap();
        common::ready(&browser, &tab).await;
        let snapshot = browser.snapshot(&tab.id).await.unwrap();
        assert!(snapshot.text.contains("Account form"));
        let png = browser.screenshot(&tab.id).await.unwrap();
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 1440);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 900);
        browser.shutdown().await.unwrap();
    }
}
