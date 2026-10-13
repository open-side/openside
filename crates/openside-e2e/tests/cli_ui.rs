//! The CLI's terminal presentation against a real daemon on a scripted model: the live run view,
//! the colour switches, and (ignored, opt-in) text captures of the main screens.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use openside_e2e::Harness;
use serde_json::{json, Value};

const DAEMON: &str = env!("CARGO_BIN_EXE_e2e-opensided");
const CLI: &str = env!("CARGO_BIN_EXE_e2e-openside");
const UTF8: &[(&str, &str)] = &[("TERM", "xterm-256color"), ("LANG", "C.UTF-8")];

fn script() -> Value {
    json!({"responses": [
        {"parts": [{"type": "tool_call", "name": "bash", "args": {"command": "cat notes.md"}, "id": "c1"}]},
        {"parts": [{"type": "tool_call", "name": "bash", "args": {"command": "cat missing.md"}, "id": "c2"}]},
        {"parts": [{"type": "text", "text":
            "Here is what I found:\n\n- **notes.md** has two items\n- `missing.md` does not exist\n\nSee [the guide](https://example.com/guide) for more.\n"}]}
    ]})
}

async fn start() -> Option<Harness> {
    let harness = Harness::start(DAEMON, &script()).await?;
    std::fs::write(
        harness.workspace().join("notes.md"),
        "- buy milk\n- call the bank\n",
    )
    .unwrap();
    Some(harness)
}

fn strip(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' {
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(character);
        }
    }
    out
}

/// What a terminal would finally show: escape codes dropped, `\r` returning to column zero so
/// the spinner line is overwritten the way a real screen overwrites it.
fn screen(bytes: &[u8]) -> String {
    let mut lines: Vec<Vec<char>> = vec![Vec::new()];
    let mut column = 0;
    for character in strip(bytes).chars() {
        match character {
            '\n' => {
                lines.push(Vec::new());
                column = 0;
            }
            '\r' => column = 0,
            _ => {
                let line = lines.last_mut().unwrap();
                line.resize(line.len().max(column), ' ');
                if column < line.len() {
                    line[column] = character;
                } else {
                    line.push(character);
                }
                column += 1;
            }
        }
    }
    lines
        .into_iter()
        .map(|line| line.into_iter().collect::<String>().trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_live_run_on_a_terminal_shows_text_tool_rows_and_a_summary() {
    let Some(mut harness) = start().await else {
        return;
    };
    let (status, bytes) = harness
        .cli_on_pty(CLI, &["read my notes"], UTF8, 80, 40)
        .await;
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let screen = screen(&bytes);
    assert!(status.success(), "{raw}");
    assert!(raw.contains('\u{1b}'), "a terminal gets colour: {raw:?}");
    assert!(
        !screen.contains("\"type\""),
        "no JSONL on a terminal: {screen}"
    );
    for expected in ["Ran a command", "does not exist", "Done"] {
        assert!(screen.contains(expected), "missing {expected:?}:\n{screen}");
    }
    assert!(screen.contains('✓'), "{screen}");
    assert!(
        screen.contains("https://example.com/guide"),
        "the link keeps its URL: {screen}"
    );
    assert!(
        !screen.contains("**"),
        "bold markers are rendered, not printed: {screen}"
    );
    harness.kill().await;
}

#[tokio::test]
async fn no_color_keeps_the_layout_and_drops_every_escape_code() {
    let Some(mut harness) = start().await else {
        return;
    };
    let env = [
        ("TERM", "xterm-256color"),
        ("LANG", "C.UTF-8"),
        ("NO_COLOR", "1"),
    ];
    let (status, bytes) = harness
        .cli_on_pty(CLI, &["read my notes"], &env, 80, 40)
        .await;
    assert!(status.success());
    assert!(
        !bytes.contains(&0x1b),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
    let screen = screen(&bytes);
    assert!(
        screen.contains("Ran a command") && screen.contains("✓ Done"),
        "{screen}"
    );
    harness.kill().await;
}

#[tokio::test]
async fn a_dumb_terminal_gets_ascii_rows_and_no_status_line() {
    let Some(mut harness) = start().await else {
        return;
    };
    let (status, bytes) = harness
        .cli_on_pty(CLI, &["read my notes"], &[("TERM", "dumb")], 80, 40)
        .await;
    assert!(status.success());
    assert!(
        !bytes.contains(&0x1b),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    assert!(text.is_ascii(), "{text:?}");
    assert!(
        !text.contains('\r'),
        "no redrawn status line on a dumb terminal: {text:?}"
    );
    assert!(text.contains("Ran a command"), "{text}");
    harness.kill().await;
}

#[tokio::test]
async fn piped_output_stays_jsonl_unless_colour_is_explicitly_forced() {
    let Some(mut harness) = start().await else {
        return;
    };
    let piped = harness.cli(CLI, &["--color", "never", "one"], None).await;
    assert!(piped.status.success());
    let last = String::from_utf8_lossy(&piped.stdout)
        .lines()
        .last()
        .unwrap()
        .to_owned();
    assert!(
        serde_json::from_str::<Value>(&last).unwrap()["type"] == "summary",
        "{last}"
    );
    assert!(!piped.stdout.contains(&0x1b));
    harness.replace_script(&script());
    let forced = harness.cli(CLI, &["--color", "always", "two"], None).await;
    assert!(
        forced.status.success(),
        "{}",
        String::from_utf8_lossy(&forced.stderr)
    );
    assert!(
        forced.stdout.contains(&0x1b),
        "{:?}",
        String::from_utf8_lossy(&forced.stdout)
    );
    assert!(!String::from_utf8_lossy(&forced.stdout).contains("\"type\""));
    harness.kill().await;
}

/// `OPENSIDE_CLI_CAPTURE_DIR=/tmp/openside-qa/round2/CliPolish cargo test -p openside-e2e --test
/// cli_ui -- --ignored`: raw ANSI captures (`*.ansi`) of the screens a person sees.
#[tokio::test]
#[ignore = "writes captures to OPENSIDE_CLI_CAPTURE_DIR"]
async fn capture_cli_screens() {
    let dir =
        PathBuf::from(std::env::var("OPENSIDE_CLI_CAPTURE_DIR").expect("OPENSIDE_CLI_CAPTURE_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let Some(mut harness) = start().await else {
        return;
    };
    let shot = |name: &str, bytes: Vec<u8>| {
        std::fs::write(dir.join(format!("{name}.ansi")), bytes).unwrap()
    };
    for (name, args, columns) in [
        ("help", vec!["--help"], 100u16),
        ("help-narrow", vec!["--help"], 60),
        ("no-args", vec![], 100),
        ("accounts", vec!["accounts"], 110),
        ("models", vec!["models"], 110),
        ("doctor", vec!["doctor"], 100),
        ("skills", vec!["skills", "list"], 100),
    ] {
        shot(
            name,
            harness.cli_on_pty(CLI, &args, UTF8, columns, 50).await.1,
        );
    }
    shot(
        "live-run",
        harness
            .cli_on_pty(CLI, &["read my notes"], UTF8, 100, 40)
            .await
            .1,
    );
    shot(
        "session-list",
        harness
            .cli_on_pty(CLI, &["session", "list"], UTF8, 100, 20)
            .await
            .1,
    );
    harness.replace_script(&script());
    let jsonl = harness.cli(CLI, &["--json", "read my notes"], None).await;
    std::fs::write(dir.join("live-run.jsonl"), jsonl.stdout).unwrap();
    harness.replace_script(&script());
    let env = [
        ("TERM", "xterm-256color"),
        ("LANG", "C.UTF-8"),
        ("NO_COLOR", "1"),
    ];
    shot(
        "live-run-no-color",
        harness
            .cli_on_pty(CLI, &["read my notes"], &env, 100, 40)
            .await
            .1,
    );
    harness.replace_script(&script());
    shot(
        "live-run-narrow",
        harness
            .cli_on_pty(CLI, &["read my notes"], UTF8, 50, 40)
            .await
            .1,
    );
    harness.kill().await;
}
