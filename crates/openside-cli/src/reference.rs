//! Markdown command reference walked from the clap definitions, so it cannot drift from `--help`.

use std::fmt::Write;

use clap::{Arg, Command, CommandFactory};

use crate::Args;

pub(crate) fn markdown() -> String {
    let mut root = Args::command();
    // Building propagates global flags and fills in every subcommand's `bin_name`, which
    // `render_usage` needs.
    root.build();
    let mut out = String::new();
    out.push_str(
        "# CLI reference\n\n\
         Generated from the `openside` command definitions by \
         `cargo run -q -p openside-cli --example cli-docs`; do not edit by hand \
         (`tools/gen-docs.sh` regenerates it).\n\n",
    );
    section(&mut out, &root, 2, true);
    out
}

fn section(out: &mut String, command: &Command, depth: usize, is_root: bool) {
    let name = command.get_bin_name().unwrap_or_else(|| command.get_name());
    let _ = writeln!(out, "{} `{name}`\n", "#".repeat(depth));
    if let Some(about) = command.get_long_about().or_else(|| command.get_about()) {
        let _ = writeln!(out, "{}\n", about.to_string().trim());
    }
    let mut usage = command.clone();
    let _ = writeln!(
        out,
        "```text\n{}\n```\n",
        usage.render_usage().to_string().trim()
    );

    let visible: Vec<&Arg> = command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && (is_root || !arg.is_global_set()))
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
        .collect();
    if !visible.is_empty() {
        out.push_str("| Argument | Description |\n|---|---|\n");
        for arg in visible {
            let _ = writeln!(
                out,
                "| {} | {} |",
                cell(&signature(arg)),
                cell(&details(arg))
            );
        }
        out.push('\n');
        if is_root {
            out.push_str("`--socket`, `--url`, `--token` and `--json` are global and apply to every subcommand.\n\n");
        }
    }
    for sub in command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
    {
        section(out, sub, depth + 1, false);
    }
}

fn signature(arg: &Arg) -> String {
    let value = arg
        .get_value_names()
        .and_then(|names| names.first().map(|name| name.to_string()))
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());
    if arg.is_positional() {
        let wrapped = if arg.is_required_set() {
            format!("<{value}>")
        } else {
            format!("[{value}]")
        };
        return format!("`{wrapped}`");
    }
    let mut flags = Vec::new();
    if let Some(short) = arg.get_short() {
        flags.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        flags.push(format!("--{long}"));
    }
    let takes_value = arg.get_action().takes_values();
    let mut text = flags.join(", ");
    if takes_value {
        let _ = write!(text, " <{value}>");
    }
    format!("`{text}`")
}

fn details(arg: &Arg) -> String {
    let mut parts = Vec::new();
    if let Some(help) = arg.get_long_help().or_else(|| arg.get_help()) {
        parts.push(help.to_string().trim().to_owned());
    }
    if arg.is_required_set() && !arg.is_positional() {
        parts.push("Required.".to_owned());
    }
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| format!("`{}`", value.get_name()))
        .collect();
    if arg.get_action().takes_values() && !possible.is_empty() {
        parts.push(format!("One of {}.", possible.join(", ")));
    }
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| format!("`{}`", value.to_string_lossy()))
        .collect();
    if arg.get_action().takes_values() && !defaults.is_empty() {
        parts.push(format!("Default: {}.", defaults.join(", ")));
    }
    if let Some(env) = arg.get_env() {
        parts.push(format!("Environment: `{}`.", env.to_string_lossy()));
    }
    parts.join(" ")
}

/// Table cells cannot contain pipes or line breaks.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
