//! `openside import`: copy history, bookmarks and cookies from a Chrome, Chromium or Firefox
//! profile directory into a named openside browser profile (`docs/specs/import.md`). Runs
//! without a daemon; the source profile is only read.

use std::io::Write;
use std::path::PathBuf;

use clap::{Args, ValueEnum};
use openside_import::{
    CategoryReport, ImportOptions, KeyringSource, ProfileLayout, Report, Source, What,
};

use crate::{emit, output_error};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Category {
    History,
    Bookmarks,
    Cookies,
}

/// Import a Chrome, Chromium or Firefox profile into an openside browser profile.
#[derive(Args)]
pub(crate) struct ImportArgs {
    /// Source profile as `<chrome|chromium|firefox>:<profile directory>`.
    #[arg(long, value_name = "BROWSER:DIR")]
    from: String,
    /// openside browser profile to write (1-64 characters of a-z, 0-9, `_`, `-`).
    #[arg(long, value_name = "PROFILE")]
    into: String,
    /// Comma-separated categories to import; default is all that the source has.
    #[arg(long, value_enum, value_delimiter = ',', value_name = "CATEGORY")]
    what: Vec<Category>,
    /// Read and decrypt the source and report counts, but write nothing.
    #[arg(long)]
    dry_run: bool,
    /// Command that prints the browser's OS-keyring `Safe Storage` password, needed for v11
    /// cookies. Run directly, without a shell; its output is never logged.
    #[arg(long, value_name = "COMMAND")]
    keyring_password_cmd: Option<String>,
    /// Configuration file (data directory and browser location).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

pub(crate) async fn run<W: Write>(
    args: &ImportArgs,
    json: bool,
    stdout: &mut W,
) -> Result<(), String> {
    let source: Source = args.from.parse().map_err(|error| format!("{error}"))?;
    let loaded =
        openside_config::load(args.config.as_deref()).map_err(|error| error.to_string())?;
    let target = ProfileLayout::new(&loaded.paths.data_dir, &args.into)
        .map_err(|error| error.to_string())?;
    let what = if args.what.is_empty() {
        What::ALL
    } else {
        What {
            history: args.what.contains(&Category::History),
            bookmarks: args.what.contains(&Category::Bookmarks),
            cookies: args.what.contains(&Category::Cookies),
        }
    };
    let report = openside_import::import(ImportOptions {
        source,
        target,
        what,
        skip_missing: args.what.is_empty(),
        dry_run: args.dry_run,
        keyring: args
            .keyring_password_cmd
            .clone()
            .map_or(KeyringSource::None, KeyringSource::Command),
        browser: openside_config::discover_browser(&loaded.config).ok(),
    })
    .await
    .map_err(|error| error.to_string())?;
    if json {
        return emit(stdout, &report);
    }
    print(stdout, &report)
}

fn line<W: Write>(stdout: &mut W, name: &str, category: &CategoryReport) -> Result<(), String> {
    if let Some(note) = &category.note {
        return writeln!(stdout, "  {name}: skipped ({note})").map_err(output_error);
    }
    let mut text = format!("  {name}: {} imported", category.imported);
    if category.skipped > 0 {
        text.push_str(&format!(", {} skipped", category.skipped));
    }
    writeln!(stdout, "{text}").map_err(output_error)?;
    for rejected in &category.rejected {
        writeln!(
            stdout,
            "    browser rejected cookie `{}` for {}",
            rejected.name, rejected.host
        )
        .map_err(output_error)?;
    }
    Ok(())
}

fn print<W: Write>(stdout: &mut W, report: &Report) -> Result<(), String> {
    let heading = if report.dry_run {
        format!(
            "Dry run, nothing written. Would import from {} into profile `{}`:",
            report.source, report.profile
        )
    } else {
        format!(
            "Imported from {} into profile `{}`:",
            report.source, report.profile
        )
    };
    writeln!(stdout, "{heading}").map_err(output_error)?;
    for (name, category) in [
        ("history", &report.history),
        ("bookmarks", &report.bookmarks),
        ("cookies", &report.cookies),
    ] {
        if let Some(category) = category {
            line(stdout, name, category)?;
        }
    }
    Ok(())
}
