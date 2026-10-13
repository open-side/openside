//! `--help` and usage errors: clap's own layout with the product's colours and a brand line.

use clap::{
    builder::{
        styling::{AnsiColor, Effects},
        Styles,
    },
    error::ErrorKind,
};

use super::{Role, Ui};

/// Colours for clap's headings, usage, flags and placeholders; applied only when colour is on.
pub(crate) fn clap_styles() -> Styles {
    let accent = AnsiColor::BrightBlue.on_default() | Effects::BOLD;
    Styles::styled()
        .header(accent)
        .usage(accent)
        .literal(Effects::BOLD.into())
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::Red.on_default() | Effects::BOLD)
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Yellow.on_default())
}

/// A clap help or usage error as the text to print, with the brand line above help.
pub(crate) fn render_help_error(error: &clap::Error, ui: &Ui) -> String {
    let ui = if error.use_stderr() { ui.err() } else { *ui };
    let rendered = error.render();
    let body = if ui.color {
        rendered.ansi().to_string()
    } else {
        rendered.to_string()
    };
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            format!(
                "\n  {}  {}\n\n{body}",
                ui.brand(),
                ui.paint(Role::Dim, concat!("v", env!("CARGO_PKG_VERSION"))),
            )
        }
        _ => body,
    }
}
