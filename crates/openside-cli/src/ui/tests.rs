//! Rendering snapshots with colour forced on and off.

use openside_config::doctor::Status;
use serde_json::{json, Value};

use super::{
    live::{Live, Stderr},
    markdown::Markdown,
    relative, scan_color,
    table::{Cell, Column, Table},
    text::{truncate, visible_width, width, wrap_plain},
    toolcopy::{describe, Facts},
    ColorWhen, Env, Terminal, Ui,
};

const NOW: i64 = 1_800_000_000;

fn strip(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
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

fn on(width: usize) -> Ui {
    Ui::fixed(true, true, width)
}

fn off(width: usize) -> Ui {
    Ui::fixed(false, true, width)
}

fn from_json<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

// --- width, wrapping ---------------------------------------------------------------------------

#[test]
fn width_counts_wide_characters_and_skips_escape_codes() {
    assert_eq!(width("abc"), 3);
    assert_eq!(width("한글"), 4);
    assert_eq!(visible_width("\u{1b}[1mab\u{1b}[0m한"), 4);
    assert_eq!(truncate("안녕하세요", 7, "…"), "안녕하…");
    assert_eq!(truncate("short", 9, "…"), "short");
    assert_eq!(truncate("abcdef", 4, "..."), "a...");
}

#[test]
fn wrapping_breaks_on_words_and_splits_overlong_words() {
    assert_eq!(
        wrap_plain("the quick brown fox", 10, 10),
        ["the quick", "brown fox"]
    );
    assert_eq!(wrap_plain("abcdefghij", 4, 4), ["abcd", "efgh", "ij"]);
    assert_eq!(
        wrap_plain("한국어 문장을 줄바꿈", 9, 9),
        ["한국어", "문장을", "줄바꿈"]
    );
    assert_eq!(wrap_plain("", 10, 10), [""]);
}

#[test]
fn relative_times_round_to_the_nearest_unit() {
    assert_eq!(relative(NOW, NOW - 10), "just now");
    assert_eq!(relative(NOW, NOW - 120), "2m ago");
    assert_eq!(relative(NOW, NOW - 7_200), "2h ago");
    assert_eq!(relative(NOW, NOW - 3 * 86_400), "3d ago");
    assert_eq!(relative(NOW, NOW + 3_600), "in 1h");
    assert_eq!(relative(NOW, NOW + 10), "in a moment");
    assert_eq!(relative(NOW, 1_700_000_000), "2023-11-14");
}

// --- capability detection ----------------------------------------------------------------------

fn tty() -> Terminal {
    Terminal {
        stdout_tty: true,
        stderr_tty: true,
        width: Some(100),
    }
}

fn piped() -> Terminal {
    Terminal {
        stdout_tty: false,
        stderr_tty: false,
        width: None,
    }
}

fn utf8() -> Env {
    Env {
        utf8: true,
        ..Env::default()
    }
}

#[test]
fn a_pipe_keeps_the_plain_output_unless_colour_is_forced() {
    assert!(Ui::detect(ColorWhen::Auto, &utf8(), &piped()).is_none());
    assert!(Ui::detect(ColorWhen::Never, &utf8(), &piped()).is_none());
    let forced = Ui::detect(ColorWhen::Always, &utf8(), &piped()).unwrap();
    assert!(forced.color && forced.err_color && !forced.live);
    let env = Env {
        clicolor_force: true,
        ..utf8()
    };
    assert!(Ui::detect(ColorWhen::Auto, &env, &piped()).unwrap().color);
    // NO_COLOR beats CLICOLOR_FORCE, so a forced pipe stays plain.
    let env = Env {
        clicolor_force: true,
        no_color: true,
        ..utf8()
    };
    assert!(Ui::detect(ColorWhen::Auto, &env, &piped()).is_none());
}

#[test]
fn a_terminal_gets_colour_unless_the_environment_or_flag_says_no() {
    let ui = Ui::detect(ColorWhen::Auto, &utf8(), &tty()).unwrap();
    assert!(ui.color && ui.err_color && ui.unicode && ui.live);
    assert_eq!(ui.width, 100);
    let no_color = Env {
        no_color: true,
        ..utf8()
    };
    let ui = Ui::detect(ColorWhen::Auto, &no_color, &tty()).unwrap();
    assert!(
        !ui.color && !ui.err_color && ui.live,
        "the layout stays, the colour goes"
    );
    assert!(Ui::detect(ColorWhen::Never, &utf8(), &tty()).is_some_and(|ui| !ui.color));
    let off = Env {
        clicolor_off: true,
        ..utf8()
    };
    assert!(!Ui::detect(ColorWhen::Auto, &off, &tty()).unwrap().color);
    assert!(
        Ui::detect(ColorWhen::Always, &no_color, &tty())
            .unwrap()
            .color
    );
}

#[test]
fn a_dumb_terminal_gets_ascii_and_no_status_line() {
    let env = Env {
        term_dumb: true,
        ..utf8()
    };
    let ui = Ui::detect(ColorWhen::Auto, &env, &tty()).unwrap();
    assert!(!ui.color && !ui.unicode && !ui.live);
    let ascii = Ui::detect(ColorWhen::Auto, &Env::default(), &tty()).unwrap();
    assert!(
        !ascii.unicode,
        "a non-UTF-8 locale falls back to ASCII glyphs"
    );
}

#[test]
fn width_comes_from_the_terminal_then_columns_then_80_and_is_clamped() {
    let term = Terminal {
        width: None,
        ..tty()
    };
    assert_eq!(
        Ui::detect(ColorWhen::Auto, &utf8(), &term).unwrap().width,
        80
    );
    let env = Env {
        columns: Some(132),
        ..utf8()
    };
    assert_eq!(Ui::detect(ColorWhen::Auto, &env, &term).unwrap().width, 132);
    let env = Env {
        columns: Some(10),
        ..utf8()
    };
    assert_eq!(Ui::detect(ColorWhen::Auto, &env, &term).unwrap().width, 40);
}

#[test]
fn colour_is_read_from_argv_before_clap_parses() {
    let argv = |args: &[&str]| {
        args.iter()
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        scan_color(&argv(&["openside", "--color", "always", "accounts"])),
        ColorWhen::Always
    );
    assert_eq!(
        scan_color(&argv(&["openside", "--color=never"])),
        ColorWhen::Never
    );
    assert_eq!(
        scan_color(&argv(&["openside", "--color", "bogus"])),
        ColorWhen::Auto
    );
    assert_eq!(
        scan_color(&argv(&["openside", "--", "--color=always"])),
        ColorWhen::Auto
    );
}

// --- tables ------------------------------------------------------------------------------------

fn sample_table(ui: &Ui) -> Table {
    let mut table = Table::new(vec![Column::new("NAME"), Column::new("NOTE").flex(4)]);
    table.row(vec![Cell::text("alpha"), Cell::text("first")]);
    table.row(vec![
        Cell::role(ui, super::Role::Strong, "beta-longer"),
        Cell::text("second note"),
    ]);
    table
}

#[test]
fn tables_align_columns_under_a_rule_without_trailing_spaces() {
    let ui = off(80);
    assert_eq!(
        sample_table(&ui).render(&ui),
        "  NAME         NOTE\n  ────────────────────────\n  alpha        first\n  beta-longer  second note\n"
    );
}

#[test]
fn flexible_columns_shrink_with_an_ellipsis_when_the_terminal_is_narrow() {
    let ui = off(20);
    assert_eq!(
        sample_table(&ui).render(&ui),
        "  NAME         NOTE\n  ──────────────────\n  alpha        first\n  beta-longer  seco…\n"
    );
    let ascii = Ui::fixed(false, false, 20);
    assert!(sample_table(&ascii)
        .render(&ascii)
        .contains("beta-longer  se..."));
}

#[test]
fn coloured_tables_style_the_header_and_cells_and_strip_back_to_the_plain_text() {
    let colored = sample_table(&on(80)).render(&on(80));
    assert!(colored.contains("\u{1b}[2mNAME\u{1b}[0m"), "{colored:?}");
    assert!(
        colored.contains("\u{1b}[1mbeta-longer\u{1b}[0m"),
        "{colored:?}"
    );
    assert_eq!(strip(&colored), sample_table(&off(80)).render(&off(80)));
}

// --- markdown ----------------------------------------------------------------------------------

const MARKDOWN: &str = "# Title\n\nSome **bold** and `code` with [a link](https://x.y).\n\n- one\n- two\n\n```rs\nlet a = 1;\n```\n";

#[test]
fn markdown_without_colour_keeps_its_markers() {
    let mut md = Markdown::new(off(80));
    let out = md.push(MARKDOWN) + &md.finish();
    assert_eq!(
        out,
        "  Title\n\n  Some **bold** and `code` with a link (https://x.y).\n\n  • one\n  • two\n\n  rs\n  ▎ let a = 1;\n"
    );
}

#[test]
fn markdown_with_colour_styles_runs_and_shows_links_as_text_and_dim_url() {
    let mut md = Markdown::new(on(80));
    let out = md.push(MARKDOWN) + &md.finish();
    assert!(out.contains("\u{1b}[1mbold\u{1b}[0m"), "{out:?}");
    assert!(out.contains("\u{1b}[36mcode\u{1b}[0m"), "{out:?}");
    assert!(
        out.contains("\u{1b}[4ma\u{1b}[0m \u{1b}[4mlink\u{1b}[0m \u{1b}[2m(https://x.y)\u{1b}[0m"),
        "{out:?}"
    );
    assert!(out.contains("\u{1b}[94m•\u{1b}[0m one"), "{out:?}");
    assert!(!out.contains("**"));
}

#[test]
fn markdown_renders_a_line_only_once_its_newline_arrives() {
    let mut md = Markdown::new(off(80));
    assert_eq!(md.push("Hel"), "");
    assert!(md.has_pending());
    assert_eq!(md.push("lo\nwor"), "  Hello\n");
    assert_eq!(md.finish(), "  wor\n");
    assert!(!md.has_pending());
}

#[test]
fn markdown_wraps_paragraphs_and_hangs_list_items_at_the_terminal_width() {
    let mut md = Markdown::new(off(30));
    let out = md.push("alpha beta gamma delta epsilon zeta eta theta\n\n1. first item that is long enough to wrap\n");
    for line in out.lines() {
        assert!(visible_width(line) <= 30, "{line:?}");
    }
    assert!(out.contains("\n  1. first item that is long"), "{out}");
    assert!(
        out.contains("\n     enough to wrap") || out.contains("\n     long enough to wrap"),
        "{out}"
    );
}

#[test]
fn markdown_leaves_snake_case_and_escapes_alone() {
    let mut md = Markdown::new(on(80));
    let out = md.push("use my_var_name and 2 * 3 * 4 and \\*literal\\*\n");
    assert_eq!(
        strip(&out),
        "  use my_var_name and 2 * 3 * 4 and *literal*\n"
    );
}

// --- tool copy ---------------------------------------------------------------------------------

#[test]
fn tool_phrases_match_the_web_copy() {
    let facts = Facts {
        host: Some("acme.com".into()),
        title: Some("Acme — Sign in".into()),
        label: Some("Email address".into()),
    };
    let none = Facts::default();
    let open = json!({"url": "https://www.acme.com/login"});
    assert_eq!(
        describe("browser_navigate", &open, &none, false).title,
        "Opening acme.com"
    );
    let done = describe("browser_navigate", &open, &facts, true);
    assert_eq!(
        (done.title.as_str(), done.detail.as_deref()),
        ("Opened Acme — Sign in", Some("acme.com"))
    );
    let typed = json!({"action": "type", "submit": true});
    assert_eq!(
        describe("browser_act", &typed, &facts, true).title,
        "Filled the email address field and submitted"
    );
    assert_eq!(
        describe("browser_act", &json!({"action": "click"}), &facts, false).title,
        "Clicking “Email address”"
    );
    assert_eq!(
        describe("read", &json!({"path": "/tmp/notes/todo.md"}), &none, true).title,
        "Read todo.md"
    );
    assert_eq!(
        describe("bash", &json!({"command": "ls -la\nwc"}), &none, true)
            .detail
            .as_deref(),
        Some("ls -la")
    );
    assert_eq!(
        describe("my_server__fetch_page", &json!({}), &none, true).title,
        "Used fetch page"
    );
    assert_eq!(
        describe("skill", &json!({"name": "pdf"}), &none, false).title,
        "Loading the pdf skill"
    );
}

// --- list renderings ---------------------------------------------------------------------------

fn session_list() -> openside_proto::SessionList {
    let session = |id: &str, title: &str, status: &str, model: &str, activity: &str| {
        json!({"id": id, "title": title, "model": model, "mode": "guard", "workspace": null,
            "status": status, "activity": activity, "last_seq": 3, "queue": [], "pinned": false,
            "archived": false, "incognito": false, "browser_space": "shared"})
    };
    from_json(json!({"sessions": [
        session("ses_a1", "Plan trip", "running", "anthropic:claude-sonnet", "Reading a page"),
        session("ses_b2", "Pay the electricity bill", "waiting_human", "openai:gpt", ""),
        session("ses_c3", "Old chat", "idle", "openai:gpt", ""),
    ]}))
}

fn ticket_json() -> Value {
    json!({"id": "t-1", "session_id": "ses_a1", "tool_call_id": "call-1",
        "deferral": "approval", "kind": "confirm", "summary": "Run tool?", "instructions": null,
        "options": [{"value": "approve", "label": "Approve"}, {"value": "reject", "label": "Reject"}],
        "origin": {"subagent_id": null}, "tool": {"name": "test", "args": {}},
        "target": null, "resume_when": null, "created_at": "2026-01-01T00:00:00Z",
        "expires_at": "2099-01-01T00:00:00Z", "status": "pending", "resolution": null})
}

type Render = Box<dyn Fn(&Ui) -> String>;

fn cases() -> Vec<(&'static str, Render)> {
    let accounts: openside_proto::AccountsResponse = from_json(json!({
    "cached_for_ms": 0, "truncated": false,
    "sources": [
        {"name": "claude-code", "kind": "subscription", "available": true, "reason": "",
         "accounts": [{"id": "a1", "label": "me@example.com", "provider": "anthropic",
            "models": ["claude-sonnet", "claude-opus"],
            "usage_windows": [{"name": "5h", "used_percent": 42.0, "reset_at_ms": null},
                               {"name": "7d", "used_percent": 93.5, "reset_at_ms": null}],
            "blocked_until_ms": null}]},
        {"name": "codex", "kind": "subscription", "available": false, "reason": "not signed in", "accounts": []},
    ]}));
    let devices: openside_proto::DeviceList = from_json(json!({"devices": [
        {"device_id": "dev_phone_1234567890", "name": "Pixel", "scopes": ["read", "steer", "approve", "human_step", "admin"],
         "kind": "person_device", "created_at": "2026-01-01T00:00:00Z"}]}));
    let skills: openside_proto::SkillList = from_json(json!({"skills": [
        {"name": "pdf", "description": "Fill in and read PDF forms", "source": "user", "enabled": true},
        {"name": "notes", "description": "Project notes", "source": "project", "enabled": false}], "problems": []}));
    let routines: openside_proto::RoutineList = from_json(json!({"routines": [
        {"id": "r1", "name": "Morning digest", "schedule": {"type": "cron", "expr": "0 8 * * *", "timezone": "Asia/Seoul"},
         "action": {"type": "new_session", "prompt": "Summarise my inbox"}, "enabled": true, "paused_reason": null,
         "catch_up": false, "created_at": "2026-01-01T00:00:00Z", "next_run_at": "2099-01-01T00:00:00Z"}]}));
    let tasks: openside_proto::SubagentList = from_json(json!({"subagents": [
        {"id": "sub_1", "parent_tool_call_id": "c1", "task": "Compare prices", "model": "m", "tools": [],
         "status": "running", "started_at": "2026-01-01T00:00:00Z", "ended_at": null, "reason": null,
         "last_progress": null, "background": true, "output": null}]}));
    let keys: openside_proto::ProviderKeyList = from_json(json!({"keys": [
        {"provider": "openai", "key_id":"key1", "label":"Work", "enabled":true, "reserve_percent":10, "last_four": "ab12", "created_at": "2026-01-01T00:00:00Z"}]}));
    let models: openside_proto::ModelCatalog = from_json(json!({
        "providers": [{"provider": "anthropic", "pool":{"provider":"anthropic","accounts":[]}, "models": [
            {"id": "claude-sonnet", "label": "Claude Sonnet", "available": true, "reason": "",
             "source": "claude-code", "account_id": "a1", "accounts": []},
            {"id": "claude-opus", "label": "Claude Opus", "available": false, "reason": "no credit",
             "source": null, "account_id": null, "accounts": []}]}],
        "default_model": "anthropic:claude-sonnet", "fallback_models": [],
        "cached_for_ms": 0, "truncated": false}));
    let tickets: openside_proto::TicketList = from_json(json!({"tickets": [ticket_json()]}));
    let doctor = openside_config::doctor::DoctorReport {
        checks: vec![
            check("daemon", Status::Ok, "Daemon is running", None),
            check(
                "config",
                Status::Warn,
                "No default model is configured",
                Some("Run `openside models set <model>`"),
            ),
            check(
                "sandbox",
                Status::Fail,
                "The sandbox could not start",
                Some("Install bubblewrap, then run `openside doctor` again"),
            ),
        ],
    };
    vec![
        (
            "sessions",
            Box::new(move |ui| super::sessions(ui, &session_list())),
        ),
        (
            "accounts",
            Box::new(move |ui| super::accounts(ui, &accounts, NOW)),
        ),
        (
            "devices",
            Box::new(move |ui| super::devices(ui, &devices, NOW)),
        ),
        ("skills", Box::new(move |ui| super::skills(ui, &skills))),
        (
            "routines",
            Box::new(move |ui| super::routines(ui, &routines, NOW)),
        ),
        ("tasks", Box::new(move |ui| super::tasks(ui, &tasks))),
        ("keys", Box::new(move |ui| super::key_list(ui, &keys))),
        ("models", Box::new(move |ui| super::models(ui, &models))),
        (
            "tickets",
            Box::new(move |ui| super::tickets(ui, &tickets, NOW)),
        ),
        ("doctor", Box::new(move |ui| super::doctor(ui, &doctor))),
    ]
}

fn check(
    id: &str,
    status: Status,
    summary: &str,
    next: Option<&str>,
) -> openside_config::doctor::Check {
    openside_config::doctor::Check {
        id: id.into(),
        status,
        summary: summary.into(),
        next_step: next.map(str::to_owned),
    }
}

#[test]
fn every_rendering_is_the_same_text_with_and_without_colour() {
    let cases = cases();
    assert!(cases.len() >= 8);
    for (name, render) in cases {
        let plain = render(&off(100));
        let colored = render(&on(100));
        assert!(colored.contains('\u{1b}'), "{name} has no colour");
        assert!(
            !plain.contains('\u{1b}'),
            "{name} leaks escape codes without colour"
        );
        assert_eq!(strip(&colored), plain, "{name}");
        for line in plain.lines() {
            assert!(visible_width(line) <= 100, "{name}: {line:?}");
            assert_eq!(line, line.trim_end(), "{name}: trailing space in {line:?}");
        }
        let narrow = render(&off(80));
        for line in narrow.lines() {
            assert!(visible_width(line) <= 80, "{name} at 80 columns: {line:?}");
        }
        let ascii = render(&Ui::fixed(false, false, 100));
        assert!(
            ascii.is_ascii(),
            "{name} is not ASCII without Unicode:\n{ascii}"
        );
    }
}

#[test]
fn the_session_list_shows_status_ids_titles_and_activity() {
    let ui = off(100);
    let text = super::sessions(&ui, &session_list());
    assert_eq!(
        text,
        "  STATUS       ID      TITLE                      Started with\n  \
         ───────────────────────────────────────────────────────────────────────\n  \
         ● running    ses_a1  Plan trip  Reading a page  anthropic:claude-sonnet\n  \
         ◆ needs you  ses_b2  Pay the electricity bill   openai:gpt\n  \
         ○ idle       ses_c3  Old chat                   openai:gpt\n"
    );
    assert_eq!(
        super::sessions(&ui, &from_json(json!({"sessions": []}))),
        "  No sessions yet\n  Start one with: openside \"what you want done\"\n"
    );
}

// --- live view ---------------------------------------------------------------------------------

fn event(seq: u64, kind: Value) -> openside_proto::Event {
    let mut value =
        json!({"session_id": "ses_a1", "seq": seq, "ts": "2026-01-01T00:00:00Z", "run_id": null});
    for (key, entry) in kind.as_object().unwrap() {
        value[key] = entry.clone();
    }
    from_json(value)
}

fn drive(ui: Ui, events: &[Value]) -> (String, String) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        {
            let mut err = Stderr::new(&mut stderr);
            let mut live = Live::new(ui);
            live.begin("ses_a1", &mut err).unwrap();
            for (index, kind) in events.iter().enumerate() {
                live.event(
                    &event(index as u64 + 1, kind.clone()),
                    &mut stdout,
                    &mut err,
                )
                .unwrap();
            }
            live.finish("settled", "ses_a1", &mut stdout, &mut err)
                .unwrap();
        }
        (
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    })
}

fn run_events() -> Vec<Value> {
    vec![
        json!({"kind": "turn.started"}),
        json!({"kind": "assistant.delta", "text": "Let me **check** that"}),
        json!({"kind": "assistant.delta", "text": " page.\n"}),
        json!({"kind": "tool.started", "tool_call_id": "c1", "name": "browser_navigate", "args": {"url": "https://www.acme.com/login"}}),
        json!({"kind": "tool.ended", "tool_call_id": "c1", "result": {"title": "Acme — Sign in"}, "is_error": false}),
        json!({"kind": "tool.started", "tool_call_id": "c2", "name": "read", "args": {"path": "/etc/missing.txt"}}),
        json!({"kind": "tool.ended", "tool_call_id": "c2", "result": {"error": "no such file"}, "is_error": true}),
        json!({"kind": "usage", "model": "anthropic:claude-sonnet", "input_tokens": 1200, "output_tokens": 340}),
        json!({"kind": "assistant.final", "text": "All done.\n"}),
        json!({"kind": "turn.ended"}),
    ]
}

#[test]
fn the_live_view_writes_text_to_stdout_and_one_row_per_tool_to_stderr() {
    let (stdout, stderr) = drive(off(80), &run_events());
    // The closing assistant.final repeats what the deltas already printed, so it is not shown twice.
    assert_eq!(stdout, "\n  Let me **check** that page.\n");
    let rows: Vec<&str> = stderr.lines().collect();
    assert_eq!(rows[0], "◖▰ openside  session ses_a1");
    assert!(
        rows.contains(&"  ✓ Opened Acme — Sign in · acme.com"),
        "{stderr}"
    );
    assert!(
        rows.contains(&"  ✗ Read missing.txt · no such file"),
        "{stderr}"
    );
    let last = rows.last().unwrap();
    assert!(last.starts_with("  ✓ Done  "), "{stderr}");
    assert!(
        last.contains("anthropic:claude-sonnet · 1.2k in, 340 out"),
        "{stderr}"
    );
}

#[test]
fn the_live_view_colours_status_words_and_strips_back_to_the_plain_rows() {
    let (_, plain) = drive(off(80), &run_events());
    let (_, colored) = drive(on(80), &run_events());
    assert!(colored.contains("\u{1b}[32m✓\u{1b}[0m"), "{colored:?}");
    assert!(colored.contains("\u{1b}[31m✗\u{1b}[0m"), "{colored:?}");
    // Only the elapsed time differs run to run, and none of these calls take two seconds.
    assert_eq!(strip(&colored), plain);
}

#[test]
fn a_status_line_is_erased_before_any_other_stderr_line_and_on_drop() {
    use std::io::Write;
    let mut sink = Vec::new();
    {
        let mut err = Stderr::new(&mut sink);
        err.set_status(Some("  ⠋ Thinking"));
        writeln!(err, "row").unwrap();
        err.set_status(Some("  ⠙ Thinking"));
    }
    let text = String::from_utf8(sink).unwrap();
    let erase = format!("\r{}\r", " ".repeat(12));
    assert_eq!(text, format!("  ⠋ Thinking{erase}row\n  ⠙ Thinking{erase}"));
}

#[test]
fn tickets_render_as_a_clear_inline_prompt_with_the_command_to_answer_it() {
    let mut ticket = ticket_json();
    ticket["kind"] = json!("choice");
    ticket["deferral"] = json!("external");
    ticket["summary"] = json!("Pick how to verify");
    ticket["choice"] = json!({
        "origin": "https://nid.naver.com", "site_name": "Naver", "question": "How would you like to verify?",
        "multiple": false,
        "options": [{"id": "sms", "label": "Text me a code"}, {"id": "email", "label": "Email me a code", "description": "j***@naver.com"}]
    });
    let ticket: openside_proto::Ticket = from_json(ticket);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let text = runtime.block_on(async {
        let mut sink = Vec::new();
        {
            let mut err = Stderr::new(&mut sink);
            let mut live = Live::new(off(80));
            live.ticket(
                &ticket,
                &|action, _| format!("openside ticket {action} t-1"),
                crate::resolve_hint(&ticket),
                &mut err,
            )
            .unwrap();
        }
        String::from_utf8(sink).unwrap()
    });
    assert_eq!(
        text,
        "\n  ◆ Choice needed  t-1\n    Pick how to verify\n    Wants to: Using test\n    How would you like to verify?\n    1. Text me a code\n    \
         2. Email me a code (j***@naver.com)\n    Answer from another process: openside ticket resolve t-1\n    \
         Over HTTP, give that process OPENSIDE_TOKEN or --token.\n\n"
    );
}

#[test]
fn doctor_groups_checks_and_folds_passing_families_but_keeps_every_problem() {
    let mut checks: Vec<_> = (0..3)
        .map(|index| {
            check(
                &format!("native_messaging.path.{index}"),
                Status::Ok,
                "NativeMessagingHosts: /x (absent)",
                None,
            )
        })
        .collect();
    checks.push(check(
        "native_messaging.managed_profile",
        Status::Warn,
        "No known filename",
        Some("Use the vendor's setup"),
    ));
    checks.push(check(
        "sandbox.bwrap",
        Status::Ok,
        "bubblewrap found at /usr/bin/bwrap",
        None,
    ));
    checks.push(check(
        "remote.tls",
        Status::Fail,
        "Certificate expired",
        Some("Run `openside remote setup`"),
    ));
    checks.push(check("daemon", Status::Ok, "Daemon is running", None));
    let text = super::doctor(&off(100), &openside_config::doctor::DoctorReport { checks });
    assert!(
        text.contains("native_messaging.path.*") && text.contains("3 checks passed"),
        "{text}"
    );
    assert!(!text.contains("native_messaging.path.0"), "{text}");
    assert!(text.contains("  ! Browser and sandbox\n"), "{text}");
    assert!(text.contains("  ✗ Network and remote access\n"), "{text}");
    assert!(text.contains("→ Use the vendor's setup"), "{text}");
    assert!(text.contains("→ Run `openside remote setup`"), "{text}");
    assert!(
        text.contains("✓ 5 ok   ! 1 warning   ✗ 1 failing"),
        "{text}"
    );
    assert!(
        text.find("Daemon and config").unwrap() < text.find("Browser and sandbox").unwrap(),
        "groups keep their order: {text}"
    );
}
