//! Drives this computer's real desktop backend against one throwaway window.
//!
//! Ignored by default: it needs a desktop session and a window made for the test. Start a
//! terminal whose title is unique and that records the first key it receives, then run
//! `OPENSIDE_COMPUTER_SMOKE_TITLE=<title> cargo test -p openside-computer --test live_desktop -- --ignored`.
//! The test only ever sends input to that window.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use openside_computer::{Action, Computer, KeyCombo};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a desktop session and a throwaway window"]
async fn connects_captures_and_types_into_one_throwaway_window() {
    let title = std::env::var("OPENSIDE_COMPUTER_SMOKE_TITLE")
        .expect("OPENSIDE_COMPUTER_SMOKE_TITLE names the throwaway window");
    let computer = Computer::for_platform();

    let eligibility = computer.eligibility().await;
    println!(
        "eligibility: {}",
        serde_json::to_string(&eligibility).unwrap()
    );
    assert!(eligibility.eligible(), "{:?}", eligibility.reasons());

    let apps = computer.apps().await.unwrap();
    println!("windows: {}", apps.len());
    let target = apps
        .iter()
        .find(|app| app.window.title == title)
        .unwrap_or_else(|| panic!("no window titled {title:?}"))
        .window
        .clone();
    println!("target: {} {} pid {}", target.id, target.app, target.pid);

    let connection = computer
        .connect(
            "smoke",
            &target.id,
            &target.instance,
            &target.app,
            target.pid,
        )
        .await
        .unwrap();
    let (frame, window) = computer.capture("smoke", &connection.id).await.unwrap();
    assert!(frame.png.starts_with(b"\x89PNG"), "not a PNG");
    assert!(frame.width > 0 && frame.height > 0);
    println!(
        "frame: {}x{} ({} bytes)",
        frame.width,
        frame.height,
        frame.png.len()
    );
    computer
        .viewed("smoke", &connection.id, frame.width, frame.height, &window)
        .unwrap();

    let combo = KeyCombo::parse("x").unwrap();
    computer
        .act("smoke", &connection.id, Action::Key { combo })
        .await
        .unwrap();
    println!("sent: x");
    assert!(computer.stop("smoke").is_some());
}
