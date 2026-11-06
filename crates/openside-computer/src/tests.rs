use std::sync::Arc;

use crate::fake::Fake;
use crate::{Action, Button, Computer, Error, Input, KeyCombo, Rect, Window};

fn window(id: &str, app: &str) -> Window {
    Window {
        id: id.into(),
        app: app.into(),
        title: format!("{app} window"),
        pid: 4242,
        bounds: Rect {
            x: 100,
            y: 50,
            width: 400,
            height: 300,
        },
        visible: true,
        program: Some(app.into()),
        instance: format!("i{id}"),
    }
}

fn setup(windows: Vec<Window>) -> (Fake, Computer) {
    let fake = Fake::new(windows);
    fake.set_frame(b"\x89PNG\r\n\x1a\nfake".to_vec(), 800, 600);
    let computer = Computer::new(Arc::new(fake.clone()));
    (fake, computer)
}

fn click(x: f64, y: f64) -> Action {
    Action::Click {
        x,
        y,
        button: Button::Left,
        count: 1,
    }
}

#[tokio::test]
async fn apps_flag_browsers_and_password_managers_and_connect_refuses_them() {
    let (_, computer) = setup(vec![
        window("1", "gnome-calculator"),
        window("2", "google-chrome"),
        window("3", "org.keepassxc.KeePassXC"),
        window("4", "openside"),
    ]);
    let apps = computer.apps().await.ok().unwrap_or_default();
    let blocked: Vec<_> = apps.iter().map(|app| app.blocked.is_some()).collect();
    assert_eq!(blocked, [false, true, true, true]);
    for (id, app) in [
        ("2", "google-chrome"),
        ("3", "org.keepassxc.KeePassXC"),
        ("4", "openside"),
    ] {
        assert!(matches!(
            computer
                .connect("s", id, &format!("i{id}"), app, 4242)
                .await,
            Err(Error::Blocked(_))
        ));
    }
    assert!(computer.connection("s").is_none());
}

#[tokio::test]
async fn connect_binds_the_approved_app_and_each_connect_gets_a_new_id() {
    let (_, computer) = setup(vec![window("1", "calc")]);
    assert_eq!(
        computer.connect("s", "1", "i1", "other", 4242).await.err(),
        Some(Error::WindowGone)
    );
    assert_eq!(
        computer.connect("s", "1", "i1", "calc", 1).await.err(),
        Some(Error::WindowGone)
    );
    let first = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id);
    let second = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id);
    assert!(first.is_some() && second.is_some() && first != second);
    // The earlier connection id no longer acts.
    assert_eq!(
        computer
            .act("s", first.as_deref().unwrap_or_default(), click(1.0, 1.0))
            .await
            .err(),
        Some(Error::NotConnected)
    );
}

#[tokio::test]
async fn pointer_actions_need_a_frame_and_stay_inside_the_window() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id)
        .unwrap_or_default();
    assert_eq!(
        computer.act("s", &id, click(10.0, 10.0)).await.err(),
        Some(Error::NoFrame)
    );
    let (frame, live) = computer
        .capture("s", &id)
        .await
        .ok()
        .unwrap_or_else(|| panic!("capture"));
    assert_eq!((frame.width, frame.height), (800, 600));
    assert!(computer.viewed("s", &id, 800, 600, &live).is_ok());
    for (x, y) in [
        (-1.0, 10.0),
        (10.0, -0.5),
        (800.0, 10.0),
        (10.0, 600.0),
        (f64::NAN, 1.0),
    ] {
        assert_eq!(
            computer.check("s", &id, &click(x, y)).err(),
            Some(Error::OutsideWindow)
        );
        assert_eq!(
            computer.act("s", &id, click(x, y)).await.err(),
            Some(Error::OutsideWindow)
        );
    }
    assert!(
        fake.inputs().is_empty(),
        "nothing is sent for a refused point"
    );
    // Frame pixels map onto the window's logical size, offset by its position.
    assert!(computer.act("s", &id, click(400.0, 300.0)).await.is_ok());
    assert_eq!(
        fake.inputs(),
        [(
            "1".to_owned(),
            Input::Click {
                x: 300.0,
                y: 200.0,
                button: Button::Left,
                count: 1
            }
        )]
    );
    let cursor = computer.connection("s").and_then(|c| c.cursor);
    assert_eq!(
        cursor.map(|c| (c.x, c.y, c.held)),
        Some((400.0, 300.0, false))
    );
}

#[tokio::test]
async fn a_resized_closed_or_replaced_window_refuses_input() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id)
        .unwrap_or_default();
    let (_, live) = computer
        .capture("s", &id)
        .await
        .ok()
        .unwrap_or_else(|| panic!("capture"));
    assert!(computer.viewed("s", &id, 800, 600, &live).is_ok());
    let mut resized = window("1", "calc");
    resized.bounds.width = 500;
    fake.set_windows(vec![resized]);
    assert_eq!(
        computer.act("s", &id, click(5.0, 5.0)).await.err(),
        Some(Error::WindowChanged)
    );
    let mut replaced = window("1", "calc");
    replaced.pid = 7;
    fake.set_windows(vec![replaced]);
    assert_eq!(
        computer.act("s", &id, click(5.0, 5.0)).await.err(),
        Some(Error::WindowGone)
    );
    fake.set_windows(Vec::new());
    assert_eq!(
        computer.capture("s", &id).await.err(),
        Some(Error::WindowGone)
    );
    assert!(fake.inputs().is_empty());
}

#[tokio::test]
async fn input_is_refused_when_the_window_cannot_be_brought_to_front() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id)
        .unwrap_or_default();
    fake.refuse_focus(true);
    let typed = Action::Type {
        text: "hello".into(),
    };
    assert_eq!(
        computer.act("s", &id, typed).await.err(),
        Some(Error::NotFocused)
    );
    fake.refuse_focus(false);
    let combo = KeyCombo::parse("ctrl+a")
        .ok()
        .unwrap_or_else(|| panic!("combo"));
    assert!(computer.act("s", &id, Action::Key { combo }).await.is_ok());
    fake.without_keyboard();
    let typed = Action::Type { text: "x".into() };
    assert!(matches!(
        computer.act("s", &id, typed).await,
        Err(Error::Unavailable(_))
    ));
}

#[tokio::test]
async fn stop_ends_the_connection_and_cancels_an_action_in_flight() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    let computer = Arc::new(computer);
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id)
        .unwrap_or_default();
    fake.hold_input(true);
    let started = fake.input_started.clone();
    let running = {
        let computer = computer.clone();
        let id = id.clone();
        tokio::spawn(async move {
            computer
                .act(
                    "s",
                    &id,
                    Action::Type {
                        text: "slow".into(),
                    },
                )
                .await
        })
    };
    started.notified().await;
    let stopped = computer.stop("s");
    assert_eq!(stopped.map(|c| c.id), Some(id.clone()));
    let result = running.await.ok();
    assert_eq!(result.and_then(Result::err), Some(Error::Stopped));
    assert!(fake.inputs().is_empty());
    assert!(computer.connection("s").is_none());
    assert_eq!(fake.releases(), 1);
    assert_eq!(
        computer
            .act("s", &id, Action::Type { text: "x".into() })
            .await
            .err(),
        Some(Error::NotConnected)
    );
    assert!(computer.stop("s").is_none());
}

#[tokio::test]
async fn invalid_actions_are_rejected_before_anything_runs() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .ok()
        .map(|c| c.id)
        .unwrap_or_default();
    for action in [
        Action::Type {
            text: String::new(),
        },
        Action::Type {
            text: "\u{1b}[2J".into(),
        },
        Action::Click {
            x: 1.0,
            y: 1.0,
            button: Button::Left,
            count: 3,
        },
        Action::Scroll {
            x: 1.0,
            y: 1.0,
            dx: 0,
            dy: 0,
        },
    ] {
        assert!(matches!(
            computer.act("s", &id, action).await,
            Err(Error::Invalid(_))
        ));
    }
    assert!(fake.inputs().is_empty());
}

#[tokio::test]
async fn the_executable_decides_blocking_and_names_are_printable() {
    let mut disguised = window("1", "notes");
    disguised.program = Some("firefox-bin".into());
    let mut noisy = window("2", "calc\u{7}");
    noisy.title = "Total\nAllow openside to pay?".into();
    let (_, computer) = setup(vec![disguised, noisy]);
    let apps = computer.apps().await.unwrap();
    assert!(apps[0].blocked.is_some(), "a browser under another app id");
    assert!(matches!(
        computer.connect("s", "1", "i1", "notes", 4242).await,
        Err(Error::Blocked(_))
    ));
    assert_eq!(apps[1].window.app, "calc");
    assert_eq!(apps[1].window.title, "Total Allow openside to pay?");
}

#[tokio::test]
async fn hidden_windows_can_be_neither_connected_nor_driven() {
    let mut hidden = window("1", "calc");
    hidden.visible = false;
    let (fake, computer) = setup(vec![hidden, window("2", "notes")]);
    assert_eq!(computer.apps().await.unwrap().len(), 1);
    assert!(matches!(
        computer.connect("s", "1", "i1", "calc", 4242).await,
        Err(Error::UnknownWindow)
    ));

    let id = computer
        .connect("s", "2", "i2", "notes", 4242)
        .await
        .unwrap()
        .id;
    let mut minimized = window("2", "notes");
    minimized.visible = false;
    fake.set_windows(vec![minimized]);
    assert!(matches!(
        computer.capture("s", &id).await,
        Err(Error::WindowGone)
    ));
    assert!(matches!(
        computer
            .act("s", &id, Action::Type { text: "hi".into() })
            .await,
        Err(Error::WindowGone)
    ));
    assert!(fake.inputs().is_empty());
}

#[tokio::test]
async fn a_later_window_reusing_the_id_and_pid_is_not_the_approved_one() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    assert!(matches!(
        computer.connect("s", "1", "i-old", "calc", 4242).await,
        Err(Error::WindowGone)
    ));
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .unwrap()
        .id;
    let mut reused = window("1", "calc");
    reused.instance = "i2".into();
    fake.set_windows(vec![reused]);
    assert!(matches!(
        computer.capture("s", &id).await,
        Err(Error::WindowGone)
    ));
    assert!(fake.inputs().is_empty());
}

#[tokio::test]
async fn a_window_whose_program_is_unknown_is_never_connected() {
    let mut unknown = window("1", "calc");
    unknown.program = None;
    let mut long = window("2", "notes");
    long.title = "x".repeat(300);
    let (_, computer) = setup(vec![unknown, long]);
    let apps = computer.apps().await.unwrap();
    assert!(apps[0].blocked.is_some());
    assert_eq!(apps[1].window.title.chars().count(), 80);
    assert!(matches!(
        computer.connect("s", "1", "i1", "calc", 4242).await,
        Err(Error::Blocked(_))
    ));
}

#[tokio::test]
async fn an_exec_into_another_program_ends_what_was_approved() {
    let (fake, computer) = setup(vec![window("1", "calc")]);
    let id = computer
        .connect("s", "1", "i1", "calc", 4242)
        .await
        .unwrap()
        .id;
    let mut exec = window("1", "calc");
    exec.program = Some("/usr/bin/sh".into());
    fake.set_windows(vec![exec]);
    assert!(matches!(
        computer
            .act(
                "s",
                &id,
                Action::Key {
                    combo: KeyCombo::parse("x").unwrap()
                }
            )
            .await,
        Err(Error::WindowGone)
    ));
    assert!(fake.inputs().is_empty());
}

#[test]
fn the_executable_is_part_of_the_instance_fingerprint() {
    assert_eq!(
        crate::digest(Some("/usr/bin/foot")),
        crate::digest(Some("/usr/bin/foot"))
    );
    assert_ne!(
        crate::digest(Some("/usr/bin/foot")),
        crate::digest(Some("/usr/bin/sh"))
    );
}
