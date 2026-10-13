//! Interactive terminal output: colour, glyphs, tables, markdown and the live run view.
//!
//! Everything here is opt-in. `run_process` installs a [`Ui`] only when stdout is a terminal or
//! colour is forced (`--color always`, `CLICOLOR_FORCE`); every other path (pipes, `--json`,
//! embedded `run`) keeps the plain, tab-separated text the scripts depend on. The rendering
//! functions take the [`Ui`] explicitly so they can be tested with colour on and off.

pub(crate) mod cards;
mod help;
mod lists;
pub(crate) mod live;
mod markdown;
mod table;
mod text;
mod toolcopy;

use std::sync::OnceLock;

use anstyle::{AnsiColor, Effects, Style};
use clap::ValueEnum;

pub(crate) use help::{clap_styles, render_help_error};
pub(crate) use lists::{
    accounts, devices, doctor, key_list, models, routines, sessions, skills, tasks, tickets,
};

/// `--color`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ColorWhen {
    /// Colour when the stream is a terminal that supports it.
    Auto,
    /// Always emit ANSI colour, even into a pipe.
    Always,
    /// Never emit colour.
    Never,
}

/// What the process environment says about terminals; read once, injectable for tests.
#[derive(Clone, Debug, Default)]
pub(crate) struct Env {
    pub(crate) no_color: bool,
    pub(crate) clicolor_force: bool,
    pub(crate) clicolor_off: bool,
    pub(crate) term_dumb: bool,
    pub(crate) columns: Option<usize>,
    pub(crate) utf8: bool,
}

impl Env {
    pub(crate) fn from_process() -> Self {
        let var = |name: &str| std::env::var(name).ok();
        let non_empty = |name: &str| var(name).is_some_and(|value| !value.is_empty());
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|name| var(name))
            .find(|value| !value.is_empty())
            .unwrap_or_default()
            .to_ascii_lowercase();
        Self {
            // https://no-color.org: any non-empty value disables colour.
            no_color: non_empty("NO_COLOR"),
            clicolor_force: var("CLICOLOR_FORCE")
                .is_some_and(|value| !value.is_empty() && value != "0"),
            clicolor_off: var("CLICOLOR").is_some_and(|value| value == "0"),
            term_dumb: var("TERM").is_some_and(|value| value == "dumb"),
            columns: var("COLUMNS").and_then(|value| value.parse().ok()),
            utf8: cfg!(windows) || locale.contains("utf-8") || locale.contains("utf8"),
        }
    }
}

/// What the streams are, measured once at startup.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Terminal {
    pub(crate) stdout_tty: bool,
    pub(crate) stderr_tty: bool,
    pub(crate) width: Option<usize>,
}

impl Terminal {
    pub(crate) fn from_process() -> Self {
        use std::io::IsTerminal;
        Self {
            stdout_tty: std::io::stdout().is_terminal(),
            stderr_tty: std::io::stderr().is_terminal(),
            width: terminal_size::terminal_size_of(std::io::stdout())
                .or_else(|| terminal_size::terminal_size_of(std::io::stderr()))
                .map(|(width, _)| usize::from(width.0)),
        }
    }
}

/// Colour roles. The palette is the terminal's own 16 colours, so it follows the user's theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// The mark and the places the product speaks (prompts, headings).
    Brand,
    /// Secondary text: ids, times, rules, URLs.
    Dim,
    Ok,
    Warn,
    Fail,
    Strong,
}

/// Status glyphs; each has an ASCII fallback for terminals without Unicode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    Ok,
    Fail,
    Warn,
    Running,
    Idle,
    Waiting,
    Bullet,
    Rule,
    Ellipsis,
    Arrow,
    Quote,
    Sep,
    BarOn,
    BarOff,
}

/// How the human output is drawn on this terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ui {
    /// ANSI colour on stdout.
    pub(crate) color: bool,
    /// ANSI colour on stderr (it can be redirected separately).
    pub(crate) err_color: bool,
    pub(crate) unicode: bool,
    /// Terminal columns, clamped to a readable range.
    pub(crate) width: usize,
    /// stderr can host the single redrawn status line.
    pub(crate) live: bool,
}

static UI: OnceLock<Ui> = OnceLock::new();

/// The installed [`Ui`], `None` for plain output (pipes, `--json`, embedded runs).
pub(crate) fn pretty() -> Option<&'static Ui> {
    UI.get()
}

pub(crate) fn install(ui: Ui) {
    let _ = UI.set(ui);
}

/// The colour choice in argv, read before clap parses so even a usage error is styled.
pub(crate) fn scan_color(argv: &[std::ffi::OsString]) -> ColorWhen {
    let mut found = ColorWhen::Auto;
    let mut iter = argv.iter().skip(1);
    while let Some(arg) = iter.next() {
        let Some(arg) = arg.to_str() else { continue };
        if arg == "--" {
            break;
        }
        let value = if arg == "--color" {
            iter.next().and_then(|value| value.to_str())
        } else {
            arg.strip_prefix("--color=")
        };
        if let Some(parsed) = value.and_then(|value| ColorWhen::from_str(value, true).ok()) {
            found = parsed;
        }
    }
    found
}

impl Ui {
    /// `None` keeps the plain script-friendly output.
    pub(crate) fn detect(when: ColorWhen, env: &Env, term: &Terminal) -> Option<Self> {
        let forced = when == ColorWhen::Always
            || (when == ColorWhen::Auto && env.clicolor_force && !env.no_color);
        if !term.stdout_tty && !forced {
            return None;
        }
        let color_on = |tty: bool| match when {
            ColorWhen::Always => true,
            ColorWhen::Never => false,
            ColorWhen::Auto => {
                !env.no_color && !env.term_dumb && !env.clicolor_off && (tty || env.clicolor_force)
            }
        };
        Some(Self {
            color: color_on(term.stdout_tty),
            err_color: color_on(term.stderr_tty),
            unicode: env.utf8 && !env.term_dumb,
            width: term.width.or(env.columns).unwrap_or(80).clamp(40, 200),
            live: term.stderr_tty && !env.term_dumb,
        })
    }

    /// A fixed UI for rendering tests and captures.
    #[cfg(test)]
    pub(crate) fn fixed(color: bool, unicode: bool, width: usize) -> Self {
        Self {
            color,
            err_color: color,
            unicode,
            width,
            live: false,
        }
    }

    /// The same drawing rules for stderr.
    pub(crate) fn err(&self) -> Self {
        Self {
            color: self.err_color,
            ..*self
        }
    }

    pub(crate) fn style(&self, role: Role) -> Style {
        match role {
            Role::Brand => Style::new().fg_color(Some(AnsiColor::BrightBlue.into())),
            Role::Dim => Style::new().effects(Effects::DIMMED),
            Role::Ok => Style::new().fg_color(Some(AnsiColor::Green.into())),
            Role::Warn => Style::new().fg_color(Some(AnsiColor::Yellow.into())),
            Role::Fail => Style::new().fg_color(Some(AnsiColor::Red.into())),
            Role::Strong => Style::new().effects(Effects::BOLD),
        }
    }

    pub(crate) fn paint(&self, role: Role, text: &str) -> String {
        self.paint_style(self.style(role), text)
    }

    pub(crate) fn paint_style(&self, style: Style, text: &str) -> String {
        if !self.color || style == Style::new() || text.is_empty() {
            return text.to_owned();
        }
        format!("{}{text}{}", style.render(), style.render_reset())
    }

    pub(crate) fn glyph(&self, glyph: Glyph) -> &'static str {
        let (rich, ascii) = match glyph {
            Glyph::Ok => ("✓", "+"),
            Glyph::Fail => ("✗", "x"),
            Glyph::Warn => ("!", "!"),
            Glyph::Running => ("●", "*"),
            Glyph::Idle => ("○", "o"),
            Glyph::Waiting => ("◆", "?"),
            Glyph::Bullet => ("•", "*"),
            Glyph::Rule => ("─", "-"),
            Glyph::Ellipsis => ("…", "..."),
            Glyph::Arrow => ("→", "->"),
            Glyph::Quote => ("▎", "|"),
            Glyph::Sep => ("·", "-"),
            Glyph::BarOn => ("▰", "#"),
            Glyph::BarOff => ("▱", "."),
        };
        if self.unicode {
            rich
        } else {
            ascii
        }
    }

    pub(crate) fn glyph_painted(&self, glyph: Glyph, role: Role) -> String {
        self.paint(role, self.glyph(glyph))
    }

    /// Spinner frames, advanced by the live view's tick.
    pub(crate) fn spinner(&self, tick: usize) -> &'static str {
        const BRAILLE: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        const ASCII: [&str; 4] = ["|", "/", "-", "\\"];
        if self.unicode {
            BRAILLE[tick % BRAILLE.len()]
        } else {
            ASCII[tick % ASCII.len()]
        }
    }

    /// The brand mark (ring and tab of the app icon) and the wordmark.
    pub(crate) fn brand(&self) -> String {
        let mark = if self.unicode { "◖▰" } else { "(/" };
        format!(
            "{} {}",
            self.paint(Role::Brand, mark),
            self.paint(Role::Strong, "openside")
        )
    }

    pub(crate) fn rule(&self, width: usize) -> String {
        self.paint(Role::Dim, &self.glyph(Glyph::Rule).repeat(width))
    }

    /// A tone's glyph as a styled span (use this inside table cells: widths count span text).
    pub(crate) fn tone_span(&self, tone: Tone) -> Span {
        let (glyph, role) = match tone {
            Tone::Ok => (Glyph::Ok, Role::Ok),
            Tone::Warn => (Glyph::Warn, Role::Warn),
            Tone::Fail => (Glyph::Fail, Role::Fail),
            Tone::Idle => (Glyph::Idle, Role::Dim),
            Tone::Active => (Glyph::Running, Role::Brand),
            Tone::Waiting => (Glyph::Waiting, Role::Warn),
        };
        Span::role(self, role, self.glyph(glyph))
    }

    /// A tone's glyph in its colour.
    pub(crate) fn tone(&self, tone: Tone) -> String {
        self.render(&[self.tone_span(tone)])
    }

    /// Renders spans, wrapping style codes around each styled run.
    pub(crate) fn render(&self, spans: &[Span]) -> String {
        spans
            .iter()
            .map(|span| self.paint_style(span.style, &span.text))
            .collect()
    }
}

/// A status without prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Ok,
    Warn,
    Fail,
    Idle,
    Active,
    Waiting,
}

/// A run of text in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) text: String,
    pub(crate) style: Style,
}

impl Span {
    pub(crate) fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub(crate) fn plain(text: impl Into<String>) -> Self {
        Self::new(text, Style::new())
    }

    pub(crate) fn role(ui: &Ui, role: Role, text: impl Into<String>) -> Self {
        Self::new(text, ui.style(role))
    }
}

/// Seconds since the epoch, for relative times.
pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// "just now", "5m ago", "in 2h", or a date beyond a month.
pub(crate) fn relative(now: i64, then: i64) -> String {
    let delta = now.saturating_sub(then);
    let (future, secs) = (delta < 0, delta.unsigned_abs());
    let unit = |value: u64, suffix: &str| {
        if future {
            format!("in {value}{suffix}")
        } else {
            format!("{value}{suffix} ago")
        }
    };
    match secs {
        0..=44 if !future => "just now".to_owned(),
        0..=44 => "in a moment".to_owned(),
        45..=3_599 => unit((secs + 30) / 60, "m"),
        3_600..=86_399 => unit((secs + 1_800) / 3_600, "h"),
        86_400..=2_591_999 => unit((secs + 43_200) / 86_400, "d"),
        _ => date(then),
    }
}

/// `YYYY-MM-DD` (UTC) from a Unix timestamp, without a date crate.
fn date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// Control characters in page or model text could carry escape sequences.
pub(crate) fn clean(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_control())
        .collect()
}

/// A process stream that writes ANSI when `color` is on (translated to console calls on legacy
/// Windows consoles) and is the raw stream, byte for byte, otherwise.
pub(crate) fn stream<S>(raw: S, color: bool) -> Box<dyn std::io::Write + Send>
where
    S: anstream::stream::RawStream + anstream::stream::AsLockedWrite + Send + 'static,
{
    if color {
        Box::new(anstream::AutoStream::new(
            raw,
            anstream::ColorChoice::Always,
        ))
    } else {
        Box::new(raw)
    }
}

#[cfg(test)]
mod tests;
