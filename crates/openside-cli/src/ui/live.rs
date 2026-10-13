//! The live view of `openside "task"`: assistant text as markdown on stdout, tool activity as
//! one compact row each on stderr, and a single spinner line that is redrawn in place.
//!
//! Only the status line is ever rewritten. Every permanent line (a text block, a finished tool
//! row, a ticket notice) erases it first and the next tick draws it again, so nothing above the
//! status line flickers or reflows.

use std::{
    collections::{HashMap, HashSet},
    io::Write,
    time::{Duration, Instant},
};

use openside_proto::{BrowserState, Event, EventKind, Ticket, TicketKind};
use serde_json::Value;

use super::{
    markdown::Markdown,
    text::{truncate, visible_width, wrap_spans},
    toolcopy::{self, Facts},
    Glyph, Role, Span, Tone, Ui,
};
use crate::output_error;

/// Wraps stderr and owns the status line drawn on it. Any line written through it erases a drawn
/// status line first, and dropping it erases the line, so an error exit never leaves a spinner.
pub(crate) struct Stderr<'a, E: Write> {
    inner: &'a mut E,
    drawn: usize,
}

impl<'a, E: Write> Stderr<'a, E> {
    pub(crate) fn new(inner: &'a mut E) -> Self {
        Self { inner, drawn: 0 }
    }

    fn erase_sequence(&mut self) -> String {
        if self.drawn == 0 {
            return String::new();
        }
        // Overwrite with spaces instead of `ESC[K` so legacy Windows consoles need no CSI support.
        let blank = format!("\r{}\r", " ".repeat(self.drawn));
        self.drawn = 0;
        blank
    }

    /// Replaces the status line (`None` removes it).
    pub(crate) fn set_status(&mut self, line: Option<&str>) {
        let mut out = self.erase_sequence();
        if let Some(line) = line {
            out.push_str(line);
            self.drawn = visible_width(line);
        }
        if !out.is_empty() {
            let _ = self.inner.write_all(out.as_bytes());
            let _ = self.inner.flush();
        }
    }
}

impl<E: Write> Write for Stderr<'_, E> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let erase = self.erase_sequence();
        if erase.is_empty() {
            return self.inner.write(buf);
        }
        let mut joined = erase.into_bytes();
        joined.extend_from_slice(buf);
        self.inner.write_all(&joined)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

impl<E: Write> Drop for Stderr<'_, E> {
    fn drop(&mut self) {
        self.set_status(None);
    }
}

struct Running {
    id: String,
    name: String,
    args: Value,
    started: Instant,
}

/// Live rendering state for one followed session.
pub(crate) struct Live {
    ui: Ui,
    markdown: Markdown,
    started: Instant,
    tick: usize,
    turn_active: bool,
    saw_delta: bool,
    running: Vec<Running>,
    card_calls: HashSet<String>,
    /// Browser facts by tool call: the element a click acted on, from `browser.state`.
    labels: HashMap<String, String>,
    last_host: Option<String>,
    page_title: Option<(String, String)>,
    model: Option<String>,
    tokens_in: u64,
    tokens_out: u64,
    /// The last permanent line came from stderr (a row) / from stdout (text); decides blank lines.
    after_row: bool,
    after_text: bool,
    pub(crate) interval: tokio::time::Interval,
}

impl Live {
    pub(crate) fn new(ui: Ui) -> Self {
        let mut interval = tokio::time::interval(Duration::from_millis(90));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        Self {
            ui,
            markdown: Markdown::new(ui),
            started: Instant::now(),
            tick: 0,
            turn_active: false,
            saw_delta: false,
            running: Vec::new(),
            card_calls: HashSet::new(),
            labels: HashMap::new(),
            last_host: None,
            page_title: None,
            model: None,
            tokens_in: 0,
            tokens_out: 0,
            after_row: false,
            after_text: false,
            interval,
        }
    }

    /// Whether the spinner can be animated (a terminal on stderr).
    pub(crate) fn animated(&self) -> bool {
        self.ui.live
    }

    /// The brand line that opens a run.
    pub(crate) fn begin<E: Write>(
        &mut self,
        session_id: &str,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        let ui = self.ui.err();
        writeln!(
            err,
            "{}  {}",
            ui.brand(),
            ui.paint(Role::Dim, &format!("session {session_id}"))
        )
        .map_err(output_error)?;
        self.after_row = true;
        Ok(())
    }

    fn label(&self) -> Option<String> {
        if let Some(call) = self.running.last() {
            let facts = self.facts(&call.id, &call.name, &call.args);
            let title = toolcopy::describe(&call.name, &call.args, &facts, false).title;
            let more = self.running.len() - 1;
            return Some(if more > 0 {
                format!("{title} (+{more})")
            } else {
                title
            });
        }
        if self.markdown.has_pending() {
            return Some("Writing".to_owned());
        }
        self.turn_active.then(|| "Thinking".to_owned())
    }

    /// Draws (or clears) the spinner line.
    pub(crate) fn refresh<E: Write>(&self, err: &mut Stderr<'_, E>) {
        if !self.ui.live {
            return;
        }
        let ui = self.ui.err();
        let Some(label) = self.label() else {
            err.set_status(None);
            return;
        };
        let suffix = self
            .running
            .last()
            .map(|call| call.started.elapsed().as_secs())
            .filter(|secs| *secs >= 3)
            .map_or_else(String::new, |secs| format!("  {secs}s"));
        // One column short of the width: a line that fills the terminal wraps on some of them.
        let room = ui.width.saturating_sub(5 + suffix.len());
        let label = truncate(&label, room, ui.glyph(Glyph::Ellipsis));
        let line = format!(
            "  {} {label}{}",
            ui.paint(Role::Brand, ui.spinner(self.tick)),
            ui.paint(Role::Dim, &suffix)
        );
        err.set_status(Some(&line));
    }

    pub(crate) fn tick<E: Write>(&mut self, err: &mut Stderr<'_, E>) {
        self.tick = self.tick.wrapping_add(1);
        self.refresh(err);
    }

    fn facts(&self, id: &str, name: &str, args: &Value) -> Facts {
        let host = args
            .get("url")
            .and_then(Value::as_str)
            .and_then(toolcopy::host_of)
            .or_else(|| self.last_host.clone());
        let title = self
            .page_title
            .as_ref()
            .filter(|(page_host, _)| Some(page_host) == host.as_ref())
            .filter(|_| {
                name.starts_with("browser_")
                    && matches!(name, "browser_navigate" | "browser_open_tab")
            })
            .map(|(_, title)| title.clone());
        Facts {
            host,
            title,
            label: self.labels.get(id).cloned(),
        }
    }

    fn flush_text<W: Write, E: Write>(
        &mut self,
        stdout: &mut W,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        let rest = self.markdown.finish();
        self.write_text(&rest, stdout, err)
    }

    fn write_text<W: Write, E: Write>(
        &mut self,
        text: &str,
        stdout: &mut W,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        err.set_status(None);
        if self.after_row {
            writeln!(stdout).map_err(output_error)?;
            self.after_row = false;
        }
        stdout.write_all(text.as_bytes()).map_err(output_error)?;
        stdout.flush().map_err(output_error)?;
        self.after_text = true;
        Ok(())
    }

    fn row<E: Write>(
        &mut self,
        tone: Tone,
        spans: Vec<Span>,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        let ui = self.ui.err();
        if self.after_text {
            writeln!(err).map_err(output_error)?;
            self.after_text = false;
        }
        let glyph = ui.tone(tone);
        let room = ui.width.saturating_sub(5);
        let spans = super::text::truncate_spans(&spans, room, ui.glyph(Glyph::Ellipsis));
        writeln!(err, "  {glyph} {}", ui.render(&spans)).map_err(output_error)?;
        err.flush().map_err(output_error)?;
        self.after_row = true;
        Ok(())
    }

    fn note<E: Write>(
        &mut self,
        tone: Tone,
        text: &str,
        detail: Option<&str>,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        let ui = self.ui.err();
        let mut spans = vec![Span::plain(super::clean(text))];
        if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
            spans.push(Span::role(
                &ui,
                Role::Dim,
                format!(" {} {}", ui.glyph(Glyph::Sep), super::clean(detail)),
            ));
        }
        self.row(tone, spans, err)
    }

    fn browser_state(&mut self, state: &BrowserState) {
        if !state.host.is_empty() {
            self.last_host = Some(state.host.trim_start_matches("www.").to_owned());
            if !state.title.is_empty() {
                self.page_title = Some((
                    state.host.trim_start_matches("www.").to_owned(),
                    state.title.clone(),
                ));
            }
        }
        if let Some(action) = &state.action {
            if let Some(target) = action.target.as_deref().filter(|target| !target.is_empty()) {
                self.labels
                    .insert(action.tool_call_id.clone(), target.to_owned());
            }
        }
    }

    /// Renders one event.
    pub(crate) fn event<W: Write, E: Write>(
        &mut self,
        event: &Event,
        stdout: &mut W,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        let ui = self.ui.err();
        match &event.kind {
            EventKind::TurnStarted {} => {
                self.flush_text(stdout, err)?;
                self.turn_active = true;
                self.saw_delta = false;
            }
            EventKind::TurnEnded {} => {
                self.flush_text(stdout, err)?;
                self.turn_active = false;
            }
            EventKind::AssistantDelta { text } => {
                self.saw_delta = true;
                let rendered = self.markdown.push(text);
                self.write_text(&rendered, stdout, err)?;
            }
            EventKind::AssistantFinal { text } if !self.saw_delta => {
                let mut rendered = self.markdown.push(text);
                rendered.push_str(&self.markdown.finish());
                self.write_text(&rendered, stdout, err)?;
            }
            EventKind::Usage {
                model,
                input_tokens,
                output_tokens,
            } => {
                self.model = Some(model.clone());
                self.tokens_in += input_tokens;
                self.tokens_out += output_tokens;
            }
            EventKind::BrowserState(state) => self.browser_state(state),
            EventKind::ToolStarted {
                tool_call_id,
                name,
                args,
            } => {
                self.flush_text(stdout, err)?;
                self.running.push(Running {
                    id: tool_call_id.clone(),
                    name: name.clone(),
                    args: args.clone(),
                    started: Instant::now(),
                });
            }
            EventKind::ToolEnded {
                tool_call_id,
                result,
                is_error,
                card,
            } => {
                let projected = self.card_calls.remove(tool_call_id);
                if let Some(at) = self
                    .running
                    .iter()
                    .position(|call| call.id == *tool_call_id)
                {
                    let call = self.running.remove(at);
                    if card.is_some() || projected || (call.name == "present" && !is_error) {
                        self.refresh(err);
                        return Ok(());
                    }
                    let mut facts = self.facts(&call.id, &call.name, &call.args);
                    if facts.title.is_none() {
                        facts.title = result
                            .get("title")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .filter(|_| {
                                matches!(
                                    call.name.as_str(),
                                    "browser_navigate" | "browser_open_tab"
                                )
                            });
                    }
                    let described = toolcopy::describe(&call.name, &call.args, &facts, true);
                    let mut spans = vec![Span::plain(super::clean(&described.title))];
                    let mut extras = Vec::new();
                    // The daemon redacts some tool arguments; a placeholder is not worth a column.
                    if let Some(detail) = described
                        .detail
                        .filter(|detail| !detail.contains("[REDACTED]"))
                    {
                        extras.push(super::clean(&detail));
                    }
                    let secs = call.started.elapsed().as_secs();
                    if secs >= 2 {
                        extras.push(format!("{secs}s"));
                    }
                    if *is_error {
                        let reason = result
                            .get("error")
                            .or_else(|| result.get("message"))
                            .and_then(Value::as_str);
                        if let Some(reason) = reason {
                            extras.insert(0, super::clean(reason));
                        }
                    }
                    for extra in extras {
                        spans.push(Span::role(
                            &ui,
                            Role::Dim,
                            format!(" {} {extra}", ui.glyph(Glyph::Sep)),
                        ));
                    }
                    self.row(if *is_error { Tone::Fail } else { Tone::Ok }, spans, err)?;
                }
            }
            EventKind::ArtifactSaved { path, size, .. } => {
                self.note(
                    Tone::Ok,
                    &format!("Saved {path}"),
                    Some(&human_size(*size)),
                    err,
                )?;
            }
            EventKind::SubagentStarted {
                subagent_id, task, ..
            } => {
                self.note(
                    Tone::Active,
                    &format!("Helper {subagent_id} started"),
                    Some(task),
                    err,
                )?;
            }
            EventKind::SubagentEnded {
                subagent_id,
                status,
                reason,
                ..
            } => {
                let status_name = serde_json::to_value(status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let tone = if status_name == "completed" {
                    Tone::Ok
                } else {
                    Tone::Fail
                };
                self.note(
                    tone,
                    &format!("Helper {subagent_id} {status_name}"),
                    reason.as_deref(),
                    err,
                )?;
            }
            EventKind::TicketOpened { ticket } => {
                self.card_calls.insert(ticket.tool_call_id.clone());
                self.running.retain(|call| call.id != ticket.tool_call_id);
            }
            EventKind::CardPresented { tool_call_id, .. } => {
                self.card_calls.insert(tool_call_id.clone());
            }
            EventKind::TicketResolved { card: None, .. } => {
                self.note(Tone::Ok, "Answered", None, err)?
            }
            EventKind::TicketRejected {
                reason, card: None, ..
            } => self.note(Tone::Fail, "Rejected", Some(reason), err)?,
            EventKind::TicketExpired { card: None, .. } => {
                self.note(Tone::Warn, "Request expired", None, err)?
            }
            _ => {}
        }
        self.refresh(err);
        Ok(())
    }

    pub(crate) fn card<W: Write, E: Write>(
        &mut self,
        card: &openside_proto::Card,
        context: &super::cards::Context<'_>,
        stdout: &mut W,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        self.flush_text(stdout, err)?;
        err.set_status(None);
        if card.ticket_id.is_some() {
            write!(
                err,
                "{}",
                super::cards::render(&self.ui.err(), card, context)
            )
            .map_err(output_error)?;
            self.after_row = true;
            self.after_text = false;
        } else {
            write!(stdout, "{}", super::cards::render(&self.ui, card, context))
                .map_err(output_error)?;
            self.after_row = false;
            self.after_text = true;
        }
        Ok(())
    }

    /// A person's answer is needed: what is being asked, and how to give it.
    pub(crate) fn ticket<E: Write>(
        &mut self,
        ticket: &Ticket,
        command: &dyn Fn(&str, Option<&str>) -> String,
        resolve_hint: Result<(&'static str, Option<String>), String>,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        let ui = self.ui.err();
        let (headline, how): (&str, Vec<String>) = if let Some(approval) = &ticket.approval {
            let number = approval
                .number
                .as_deref()
                .map(|number| format!(" Tap {number} on your phone."))
                .unwrap_or_default();
            (
                "Approve on your phone",
                vec![
                    format!("Waiting{number} openside continues by itself once the site sees the approval."),
                    format!("To cancel: {}", command("reject", None)),
                ],
            )
        } else if ticket.kind == TicketKind::Confirm {
            (
                "Approval needed",
                vec!["Ask a person to approve on another paired web or phone device; this client cannot approve its own work. Waiting for the ticket.".to_owned()],
            )
        } else {
            let (action, value) = resolve_hint?;
            let mut how = vec![format!(
                "Answer from another process: {}",
                command(action, value.as_deref())
            )];
            how.push("Over HTTP, give that process OPENSIDE_TOKEN or --token.".to_owned());
            if ticket.kind == TicketKind::SecretInput {
                how.push(
                    "Pipe the secret to stdin; keep it out of argv and shell history.".to_owned(),
                );
            }
            let headline = match ticket.kind {
                TicketKind::SecretInput if ticket.otp.is_some() => "Code needed",
                TicketKind::SecretInput => "Secret needed",
                TicketKind::DeviceApproval => "Device approval needed",
                TicketKind::Info => "Question for you",
                TicketKind::Takeover => "Take over the browser",
                TicketKind::Login => "Sign-in needed",
                TicketKind::Choice => "Choice needed",
                TicketKind::Confirm => "Approval needed",
                TicketKind::Hire => "Teammate proposal",
            };
            (headline, how)
        };
        err.set_status(None);
        writeln!(err).map_err(output_error)?;
        writeln!(
            err,
            "  {} {}  {}",
            ui.glyph_painted(Glyph::Waiting, Role::Warn),
            ui.paint_style(
                ui.style(Role::Warn).effects(anstyle::Effects::BOLD),
                headline
            ),
            ui.paint(Role::Dim, ticket.id.as_str()),
        )
        .map_err(output_error)?;
        let width = ui.width.min(100).saturating_sub(4).max(20);
        let mut body = |text: &str, role: Option<Role>| -> Result<(), String> {
            let style = role.map_or_else(anstyle::Style::new, |role| ui.style(role));
            for line in wrap_spans(&[Span::new(super::clean(text), style)], width, width) {
                writeln!(err, "    {}", ui.render(&line)).map_err(output_error)?;
            }
            Ok(())
        };
        body(&ticket.summary, None)?;
        if let Some(tool) = &ticket.tool {
            let described = toolcopy::describe(&tool.name, &tool.args, &Facts::default(), false);
            body(&format!("Wants to: {}", described.title), Some(Role::Dim))?;
        }
        if let Some(choice) = &ticket.choice {
            body(&choice.question, None)?;
            for (index, option) in choice.options.iter().enumerate() {
                let line = match &option.description {
                    Some(description) => format!("{}. {} ({description})", index + 1, option.label),
                    None => format!("{}. {}", index + 1, option.label),
                };
                body(&line, None)?;
            }
        }
        if let Some(login) = &ticket.login {
            body(
                &format!("{} ({})", login.site_name, login.origin),
                Some(Role::Dim),
            )?;
        }
        for line in &how {
            body(line, Some(Role::Dim))?;
        }
        writeln!(err).map_err(output_error)?;
        err.flush().map_err(output_error)?;
        self.after_row = true;
        self.after_text = false;
        Ok(())
    }

    /// The closing line: how the run ended, how long it took and on what.
    pub(crate) fn finish<W: Write, E: Write>(
        &mut self,
        status: &str,
        session_id: &str,
        stdout: &mut W,
        err: &mut Stderr<'_, E>,
    ) -> Result<(), String> {
        self.flush_text(stdout, err)?;
        err.set_status(None);
        let ui = self.ui.err();
        let (tone, word) = match status {
            "settled" => (Tone::Ok, "Done"),
            "waiting_human" => (Tone::Waiting, "Waiting for you"),
            "stopped" => (Tone::Warn, "Stopped"),
            _ => (Tone::Fail, "Ended"),
        };
        let mut meta = vec![format_duration(self.started.elapsed())];
        if let Some(model) = &self.model {
            meta.push(model.clone());
        }
        if self.tokens_in + self.tokens_out > 0 {
            meta.push(format!(
                "{} in, {} out",
                compact(self.tokens_in),
                compact(self.tokens_out)
            ));
        }
        let sep = format!(" {} ", ui.glyph(Glyph::Sep));
        writeln!(err).map_err(output_error)?;
        writeln!(
            err,
            "  {} {}  {}",
            ui.tone(tone),
            ui.paint(Role::Strong, word),
            ui.paint(Role::Dim, &meta.join(&sep)),
        )
        .map_err(output_error)?;
        if status == "waiting_human" {
            writeln!(err, "    {}", ui.paint(Role::Dim, &format!("Session {session_id} is waiting on tickets. See them with: openside ticket list")))
                .map_err(output_error)?;
        }
        err.flush().map_err(output_error)
    }
}

fn human_size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

fn compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    match secs {
        0 => format!("{}ms", duration.as_millis().max(1)),
        1..=59 => format!("{secs}s"),
        _ => format!("{}m {}s", secs / 60, secs % 60),
    }
}

/// The next body frame of the event stream, redrawing the spinner on every tick while waiting.
pub(crate) async fn next_frame<E: Write>(
    response: &mut hyper::Response<hyper::body::Incoming>,
    live: &mut Option<Live>,
    err: &mut Stderr<'_, E>,
) -> Option<Result<hyper::body::Frame<bytes::Bytes>, hyper::Error>> {
    use http_body_util::BodyExt;
    let Some(live) = live.as_mut().filter(|live| live.animated()) else {
        return response.body_mut().frame().await;
    };
    loop {
        tokio::select! {
            frame = response.body_mut().frame() => return frame,
            _ = live.interval.tick() => live.tick(err),
        }
    }
}
