//! Terminal renderings of the list-like commands. Each takes the parsed API object and returns
//! the text to print; the plain tab-separated output stays in the command modules.

use openside_config::doctor::{group_of, Check, DoctorReport, Status, GROUPS};
use openside_proto::{
    AccountsResponse, DeviceList, ModelCatalog, ProviderKeyList, Routine, RoutineList,
    RoutineSchedule, SessionList, SessionStatus, SkillList, SubagentList, TicketKind, TicketList,
};
use serde::Serialize;

use super::{
    relative,
    table::{Cell, Column, Table},
    text::{truncate, visible_width, wrap_spans},
    Glyph, Role, Span, Tone, Ui,
};

fn enum_name<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn clean(text: &str) -> String {
    super::clean(text)
}

/// The text printed when a list has nothing in it, with the command that fills it.
fn empty(ui: &Ui, what: &str, hint: &str) -> String {
    format!(
        "  {}\n  {}\n",
        ui.paint(Role::Strong, what),
        ui.paint(Role::Dim, hint)
    )
}

fn status_cell(ui: &Ui, tone: Tone, word: &str) -> Cell {
    let role = match tone {
        Tone::Ok => Role::Ok,
        Tone::Warn | Tone::Waiting => Role::Warn,
        Tone::Fail => Role::Fail,
        Tone::Idle => Role::Dim,
        Tone::Active => Role::Brand,
    };
    Cell::spans(vec![
        ui.tone_span(tone),
        Span::plain(" "),
        Span::role(ui, role, word),
    ])
}

pub(crate) fn sessions(ui: &Ui, list: &SessionList) -> String {
    if list.sessions.is_empty() {
        return empty(
            ui,
            "No sessions yet",
            "Start one with: openside \"what you want done\"",
        );
    }
    let mut table = Table::new(vec![
        Column::new("STATUS"),
        Column::new("ID"),
        Column::new("TITLE").flex(14),
        Column::new("MODEL").flex(10),
    ]);
    for session in &list.sessions {
        let (tone, word) = match session.status {
            SessionStatus::Running => (Tone::Active, "running"),
            SessionStatus::WaitingHuman => (Tone::Waiting, "needs you"),
            SessionStatus::Stopped => (Tone::Idle, "stopped"),
            SessionStatus::Idle => (Tone::Idle, "idle"),
        };
        let mut title = vec![Span::plain(clean(&session.title))];
        if session.pinned {
            title.insert(
                0,
                Span::role(ui, Role::Brand, format!("{} ", ui.glyph(Glyph::Bullet))),
            );
        }
        if session.status == SessionStatus::Running && !session.activity.is_empty() {
            title.push(Span::role(
                ui,
                Role::Dim,
                format!("  {}", clean(&session.activity)),
            ));
        }
        let archived = session.archived;
        let id = Cell::role(ui, Role::Dim, session.id.as_str());
        let model = Cell::role(ui, Role::Dim, clean(&session.model));
        let title = if archived {
            Cell::role(
                ui,
                Role::Dim,
                title.iter().map(|s| s.text.clone()).collect::<String>(),
            )
        } else {
            Cell::spans(title)
        };
        table.row(vec![status_cell(ui, tone, word), id, title, model]);
    }
    table.render(ui)
}

fn bar(ui: &Ui, percent: Option<f64>) -> Vec<Span> {
    const SEGMENTS: usize = 8;
    let Some(percent) = percent else {
        return vec![Span::role(ui, Role::Dim, "unknown")];
    };
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * SEGMENTS as f64).round() as usize;
    let role = if clamped < 70.0 {
        Role::Ok
    } else if clamped < 90.0 {
        Role::Warn
    } else {
        Role::Fail
    };
    vec![
        Span::role(ui, role, ui.glyph(Glyph::BarOn).repeat(filled)),
        Span::role(
            ui,
            Role::Dim,
            ui.glyph(Glyph::BarOff).repeat(SEGMENTS - filled),
        ),
        Span::plain(format!(" {clamped:.0}%")),
    ]
}

pub(crate) fn accounts(ui: &Ui, response: &AccountsResponse, now: i64) -> String {
    if response.sources.is_empty() {
        return empty(
            ui,
            "No credential sources",
            "Sign in to Claude Code, Codex or add a provider key: openside key set <provider>",
        );
    }
    let mut table = Table::new(vec![
        Column::new("SOURCE").flex(8),
        Column::new("ACCOUNT").flex(8),
        Column::new("PROVIDER"),
        Column::new("STATUS"),
        Column::new("USAGE").flex(16),
        Column::new("MODELS").flex(6),
    ]);
    for source in &response.sources {
        if source.accounts.is_empty() {
            let tone = if source.available {
                Tone::Ok
            } else {
                Tone::Idle
            };
            table.row(vec![
                Cell::role(ui, Role::Strong, clean(&source.name)),
                Cell::role(ui, Role::Dim, clean(&source.reason)),
                Cell::empty(),
                status_cell(
                    ui,
                    tone,
                    if source.available {
                        "ready"
                    } else {
                        "unavailable"
                    },
                ),
                Cell::empty(),
                Cell::empty(),
            ]);
        }
        for account in &source.accounts {
            let blocked = account
                .blocked_until_ms
                .map(|ms| (ms / 1000.0) as i64)
                .filter(|until| *until > now);
            let (tone, word) = if let Some(until) = blocked {
                (Tone::Fail, format!("blocked {}", relative(now, until)))
            } else if account.available == Some(false) {
                (Tone::Idle, "unavailable".to_owned())
            } else {
                (Tone::Ok, "ready".to_owned())
            };
            let mut usage = Vec::new();
            for window in &account.usage_windows {
                if !usage.is_empty() {
                    usage.push(Span::plain("  "));
                }
                usage.push(Span::role(
                    ui,
                    Role::Dim,
                    format!("{} ", clean(&window.name)),
                ));
                usage.extend(bar(ui, window.used_percent));
            }
            table.row(vec![
                Cell::role(ui, Role::Strong, clean(&source.name)),
                Cell::text(clean(&account.label)),
                Cell::role(ui, Role::Dim, clean(&account.provider)),
                status_cell(ui, tone, &word),
                Cell::spans(usage),
                Cell::role(ui, Role::Dim, clean(&account.models.join(", "))),
            ]);
        }
    }
    let mut out = table.render(ui);
    if response.truncated {
        out.push_str(&format!(
            "\n  {}\n",
            ui.paint(
                Role::Dim,
                "Account metadata was cut at the documented limits."
            )
        ));
    }
    out
}

pub(crate) fn models(ui: &Ui, catalog: &ModelCatalog) -> String {
    let mut out = String::new();
    if catalog
        .providers
        .iter()
        .all(|provider| provider.models.is_empty())
    {
        out.push_str(&empty(
            ui,
            "No models available",
            "Sign in to a provider or add a key: openside key set <provider>",
        ));
    }
    let listed: Vec<_> = catalog
        .providers
        .iter()
        .flat_map(|provider| &provider.models)
        .collect();
    if !listed.is_empty() {
        let mut table = Table::new(vec![
            Column::new("MODEL"),
            Column::new("SOURCE"),
            Column::new("ACCOUNT").flex(8),
            Column::new("NOTE").flex(12),
        ]);
        for model in listed {
            let tone = if model.available {
                Tone::Ok
            } else {
                Tone::Idle
            };
            let mut name = vec![
                ui.tone_span(tone),
                Span::plain(" "),
                Span::plain(clean(&model.id)),
            ];
            if catalog.default_model.as_deref() == Some(model.id.as_str()) {
                name.push(Span::role(ui, Role::Brand, "  default"));
            }
            table.row(vec![
                Cell::spans(name),
                Cell::role(ui, Role::Dim, model.source.as_deref().unwrap_or("-")),
                Cell::role(ui, Role::Dim, model.account_id.as_deref().unwrap_or("-")),
                if model.available {
                    Cell::empty()
                } else {
                    Cell::role(ui, Role::Dim, clean(&model.reason))
                },
            ]);
        }
        out.push_str(&table.render(ui));
    }
    if !catalog.fallback_models.is_empty() {
        out.push_str(&format!(
            "\n  {}  {}\n",
            ui.paint(Role::Dim, "Fallbacks"),
            clean(
                &catalog
                    .fallback_models
                    .join(&format!(" {} ", ui.glyph(Glyph::Arrow)))
            )
        ));
    }
    if catalog.truncated {
        out.push_str(&format!(
            "\n  {}\n",
            ui.paint(
                Role::Dim,
                "The model list was cut at the documented limits."
            )
        ));
    }
    out
}

pub(crate) fn key_list(ui: &Ui, list: &ProviderKeyList) -> String {
    if list.keys.is_empty() {
        return empty(
            ui,
            "No provider keys saved",
            "Add one with: openside key set <provider>",
        );
    }
    let mut table = Table::new(vec![
        Column::new("PROVIDER"),
        Column::new("ACCOUNT"),
        Column::new("LABEL"),
        Column::new("KEY"),
        Column::new("ENABLED"),
        Column::new("SAVED"),
    ]);
    for key in &list.keys {
        table.row(vec![
            Cell::role(ui, Role::Strong, clean(&key.provider)),
            Cell::role(ui, Role::Dim, clean(&key.key_id)),
            Cell::role(ui, Role::Strong, clean(&key.label)),
            Cell::role(
                ui,
                Role::Dim,
                format!("{}{}", ui.glyph(Glyph::Ellipsis), clean(&key.last_four)),
            ),
            Cell::role(ui, Role::Dim, if key.enabled { "yes" } else { "no" }),
            Cell::role(ui, Role::Dim, key.created_at.to_string()),
        ]);
    }
    table.render(ui)
}

pub(crate) fn tickets(ui: &Ui, list: &TicketList, now: i64) -> String {
    if list.tickets.is_empty() {
        return empty(
            ui,
            "Nothing is waiting on you",
            "Tickets appear here when an agent needs an answer.",
        );
    }
    let mut table = Table::new(vec![
        Column::new("TICKET"),
        Column::new("KIND"),
        Column::new("SESSION").flex(8),
        Column::new("SUMMARY").flex(16),
        Column::new("EXPIRES"),
    ]);
    for ticket in &list.tickets {
        let kind = match ticket.kind {
            TicketKind::Confirm => "confirm",
            TicketKind::SecretInput => "secret",
            TicketKind::DeviceApproval => "device",
            TicketKind::Info => "info",
            TicketKind::Takeover => "takeover",
            TicketKind::Login => "login",
            TicketKind::Choice => "choice",
            TicketKind::Hire => "hire",
        };
        table.row(vec![
            Cell::role(ui, Role::Dim, ticket.id.as_str()),
            Cell::role(ui, Role::Warn, kind),
            Cell::role(ui, Role::Dim, ticket.session_id.as_str()),
            Cell::text(clean(&ticket.summary)),
            Cell::role(ui, Role::Dim, relative(now, ticket.expires_at.timestamp())),
        ]);
    }
    table.render(ui)
}

pub(crate) fn devices(ui: &Ui, list: &DeviceList, now: i64) -> String {
    if list.devices.is_empty() {
        return empty(
            ui,
            "No paired devices",
            "Pair a phone or browser with: openside pair",
        );
    }
    let mut table = Table::new(vec![
        Column::new("DEVICE").flex(10),
        Column::new("ID"),
        Column::new("KIND"),
        Column::new("SCOPES").flex(12),
        Column::new("PAIRED"),
    ]);
    for device in &list.devices {
        let scopes: Vec<String> = device.scopes.iter().map(enum_name).collect();
        let shown = if scopes.len() > 3 {
            format!("{} +{}", scopes[..3].join(", "), scopes.len() - 3)
        } else {
            scopes.join(", ")
        };
        let name = if device.name.is_empty() {
            "(unnamed)".to_owned()
        } else {
            clean(&device.name)
        };
        table.row(vec![
            Cell::role(ui, Role::Strong, name),
            Cell::role(ui, Role::Dim, clean(&device.device_id)),
            Cell::text(enum_name(&device.kind).replace('_', " ")),
            Cell::role(ui, Role::Dim, shown),
            Cell::role(
                ui,
                Role::Dim,
                device
                    .created_at
                    .map_or_else(|| "-".to_owned(), |at| relative(now, at.timestamp())),
            ),
        ]);
    }
    table.render(ui)
}

pub(crate) fn skills(ui: &Ui, list: &SkillList) -> String {
    if list.skills.is_empty() && list.problems.is_empty() {
        return empty(
            ui,
            "No skills",
            "Create one with: openside skills new <name>",
        );
    }
    let mut table = Table::new(vec![
        Column::new("SKILL"),
        Column::new("SOURCE"),
        Column::new("STATE"),
        Column::new("DESCRIPTION").flex(20),
    ]);
    for skill in &list.skills {
        let (tone, word) = if skill.enabled {
            (Tone::Ok, "enabled")
        } else {
            (Tone::Idle, "disabled")
        };
        table.row(vec![
            Cell::role(ui, Role::Strong, clean(&skill.name)),
            Cell::role(ui, Role::Dim, enum_name(&skill.source)),
            status_cell(ui, tone, word),
            Cell::text(clean(&skill.description)),
        ]);
    }
    for problem in &list.problems {
        table.row(vec![
            Cell::role(ui, Role::Strong, clean(&problem.name)),
            Cell::role(ui, Role::Dim, enum_name(&problem.source)),
            status_cell(ui, Tone::Fail, "invalid"),
            Cell::role(ui, Role::Fail, clean(&problem.error)),
        ]);
    }
    table.render(ui)
}

fn schedule_text(schedule: &RoutineSchedule) -> String {
    match schedule {
        RoutineSchedule::Cron { expr, timezone } => {
            format!("cron {expr} {}", timezone.as_deref().unwrap_or("UTC"))
        }
        RoutineSchedule::Interval { secs } => format!("every {secs}s"),
    }
}

pub(crate) fn routines(ui: &Ui, list: &RoutineList, now: i64) -> String {
    if list.routines.is_empty() {
        return empty(
            ui,
            "No routines",
            "Schedule one with: openside routine add --cron \"0 9 * * *\" --prompt \"...\"",
        );
    }
    let mut table = Table::new(vec![
        Column::new("STATE"),
        Column::new("ROUTINE").flex(12),
        Column::new("SCHEDULE").flex(12),
        Column::new("NEXT RUN"),
        Column::new("ID"),
    ]);
    for routine in &list.routines {
        table.row(routine_row(ui, routine, now));
    }
    table.render(ui)
}

fn routine_row(ui: &Ui, routine: &Routine, now: i64) -> Vec<Cell> {
    let state = if routine.enabled {
        status_cell(ui, Tone::Ok, "active")
    } else {
        let word = match routine.paused_reason {
            Some(_) => "paused (target missing)",
            None => "paused",
        };
        status_cell(ui, Tone::Idle, word)
    };
    vec![
        state,
        Cell::role(ui, Role::Strong, clean(&routine.name)),
        Cell::text(schedule_text(&routine.schedule)),
        Cell::role(
            ui,
            Role::Dim,
            routine
                .next_run_at
                .map_or_else(|| "-".to_owned(), |at| relative(now, at.timestamp())),
        ),
        Cell::role(ui, Role::Dim, clean(&routine.id)),
    ]
}

pub(crate) fn tasks(ui: &Ui, list: &SubagentList) -> String {
    if list.subagents.is_empty() {
        return empty(
            ui,
            "No tasks",
            "Tasks appear when the agent delegates work.",
        );
    }
    let mut table = Table::new(vec![
        Column::new("STATUS"),
        Column::new("TASK ID"),
        Column::new("TASK").flex(16),
    ]);
    for task in &list.subagents {
        let name = enum_name(&task.status);
        let (tone, word) = match name.as_str() {
            "running" => (Tone::Active, "running"),
            "waiting_human" => (Tone::Waiting, "needs you"),
            "completed" => (Tone::Ok, "done"),
            "cancelled" => (Tone::Idle, "cancelled"),
            _ => (Tone::Fail, "failed"),
        };
        table.row(vec![
            status_cell(ui, tone, word),
            Cell::role(ui, Role::Dim, clean(&task.id)),
            Cell::text(clean(&task.task)),
        ]);
    }
    let mut out = table.render(ui);
    for task in list.subagents.iter().filter(|task| task.output.is_some()) {
        out.push_str(&format!(
            "\n  {}\n",
            ui.paint(Role::Dim, &format!("{} result", clean(&task.id)))
        ));
        let width = ui.width.min(100).saturating_sub(4).max(20);
        for line in wrap_spans(
            &[Span::plain(clean(
                task.output.as_deref().unwrap_or_default(),
            ))],
            width,
            width,
        ) {
            out.push_str(&format!("    {}\n", ui.render(&line)));
        }
    }
    out
}

/// A line of the doctor list: one check, or a run of passing checks that share an id prefix.
enum Entry<'a> {
    Check(&'a Check),
    Passing { family: String, count: usize },
}

/// Three or more passing checks under one `family.` prefix read as noise; fold them into one line.
fn entries<'a>(checks: &[&'a Check]) -> Vec<Entry<'a>> {
    let family = |check: &Check| {
        check
            .id
            .rsplit_once('.')
            .map(|(family, _)| family.to_owned())
    };
    let passing = |family_name: &str| {
        checks
            .iter()
            .filter(|check| {
                check.status == Status::Ok && family(check).as_deref() == Some(family_name)
            })
            .count()
    };
    let mut out = Vec::new();
    let mut folded: Vec<String> = Vec::new();
    for check in checks {
        match family(check).filter(|name| check.status == Status::Ok && passing(name) >= 3) {
            Some(name) if folded.contains(&name) => {}
            Some(name) => {
                out.push(Entry::Passing {
                    count: passing(&name),
                    family: name.clone(),
                });
                folded.push(name);
            }
            None => out.push(Entry::Check(check)),
        }
    }
    out
}

fn tone_of(status: Status) -> Tone {
    match status {
        Status::Ok => Tone::Ok,
        Status::Warn => Tone::Warn,
        Status::Fail => Tone::Fail,
    }
}

pub(crate) fn doctor(ui: &Ui, report: &DoctorReport) -> String {
    let mut out = format!("\n  {}  {}\n", ui.brand(), ui.paint(Role::Dim, "doctor"));
    let groups: Vec<(&str, Vec<Entry>)> = GROUPS
        .iter()
        .map(|group| {
            let checks: Vec<_> = report
                .checks
                .iter()
                .filter(|check| group_of(&check.id) == *group)
                .collect();
            (*group, checks)
        })
        .filter(|(_, checks)| !checks.is_empty())
        .map(|(group, checks)| (group, entries(&checks)))
        .collect();
    let id_of = |entry: &Entry| match entry {
        Entry::Check(check) => check.id.clone(),
        Entry::Passing { family, .. } => format!("{family}.*"),
    };
    let id_width = groups
        .iter()
        .flat_map(|(_, entries)| entries.iter())
        .map(|entry| visible_width(&id_of(entry)))
        .max()
        .unwrap_or(0)
        .min(26);
    let summary_room = ui.width.saturating_sub(4 + 2 + id_width + 2).max(20);
    let hint_room = ui.width.saturating_sub(8).max(20);
    for (group, entries) in &groups {
        let worst = entries
            .iter()
            .map(|entry| match entry {
                Entry::Check(check) => check.status,
                Entry::Passing { .. } => Status::Ok,
            })
            .fold(Status::Ok, |worst, status| match (worst, status) {
                (Status::Fail, _) | (_, Status::Fail) => Status::Fail,
                (Status::Warn, _) | (_, Status::Warn) => Status::Warn,
                _ => Status::Ok,
            });
        out.push_str(&format!(
            "\n  {} {}\n",
            ui.tone(tone_of(worst)),
            ui.paint(Role::Strong, group)
        ));
        for entry in entries {
            let id = truncate(&id_of(entry), id_width, ui.glyph(Glyph::Ellipsis));
            let pad = " ".repeat(id_width.saturating_sub(visible_width(&id)));
            let check = match entry {
                Entry::Passing { count, .. } => {
                    out.push_str(&format!(
                        "    {} {}{pad}  {}\n",
                        ui.tone(Tone::Ok),
                        ui.paint(Role::Dim, &id),
                        ui.paint(Role::Dim, &format!("{count} checks passed"))
                    ));
                    continue;
                }
                Entry::Check(check) => check,
            };
            let summary = clean(&check.summary);
            // A passing check is one quiet line; the full sentence is for what needs attention.
            let lines = if check.status == Status::Ok {
                vec![vec![Span::plain(truncate(
                    &summary,
                    summary_room,
                    ui.glyph(Glyph::Ellipsis),
                ))]]
            } else {
                wrap_spans(&[Span::plain(summary)], summary_room, summary_room)
            };
            for (index, line) in lines.iter().enumerate() {
                if index == 0 {
                    out.push_str(&format!(
                        "    {} {}{pad}  {}\n",
                        ui.tone(tone_of(check.status)),
                        ui.paint(Role::Dim, &id),
                        ui.render(line)
                    ));
                } else {
                    out.push_str(&format!(
                        "    {}  {}\n",
                        " ".repeat(id_width + 2),
                        ui.render(line)
                    ));
                }
            }
            if let Some(next) = check
                .next_step
                .as_deref()
                .filter(|_| check.status != Status::Ok)
            {
                for (index, line) in wrap_spans(&[Span::plain(clean(next))], hint_room, hint_room)
                    .iter()
                    .enumerate()
                {
                    let lead = if index == 0 {
                        ui.glyph(Glyph::Arrow)
                    } else {
                        " "
                    };
                    let role = if check.status == Status::Fail {
                        Role::Fail
                    } else {
                        Role::Warn
                    };
                    out.push_str(&format!(
                        "      {} {}\n",
                        ui.paint(role, lead),
                        ui.paint(Role::Dim, &ui.render(line))
                    ));
                }
            }
        }
    }
    let count = |status: Status| {
        report
            .checks
            .iter()
            .filter(|check| check.status == status)
            .count()
    };
    let (ok, warn, fail) = (count(Status::Ok), count(Status::Warn), count(Status::Fail));
    out.push_str(&format!(
        "\n  {}\n  {} {ok} ok   {} {warn} warning{}   {} {fail} failing\n",
        ui.rule(ui.width.min(60).saturating_sub(2)),
        ui.tone(Tone::Ok),
        ui.tone(Tone::Warn),
        if warn == 1 { "" } else { "s" },
        ui.tone(Tone::Fail),
    ));
    out
}
