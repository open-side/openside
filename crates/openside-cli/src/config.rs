//! `openside config schema|get|set|edit`: every `config.toml` key through
//! `/v1/settings/config`. The daemon validates each write with the file's own parser and applies
//! the local-only rule (away from the computer, a paired device may change only the behaviour
//! keys it lists in `remote_editable`); the file-only `credentials` section is never shown or
//! written. Changes apply when the daemon restarts.

use std::{io::Write, path::Path};

use clap::Subcommand;
use serde_json::{json, Map, Value};

use crate::{client::Client, emit, output_error};

const CONFIG: &str = "/v1/settings/config";

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// Print the JSON schema of config.toml (every key, its type and its help).
    Schema,
    /// Print one key by its dotted path, or the whole document without a key.
    Get {
        /// Dotted path, e.g. tickets.confirm_ttl_secs.
        key: Option<String>,
    },
    /// Change one key. The daemon validates it like config.toml and refuses local-only keys
    /// away from this computer.
    Set {
        /// Dotted path, e.g. tickets.confirm_ttl_secs.
        key: String,
        /// JSON (30, true, ["a","b"], null) or plain text for a text key.
        value: String,
    },
    /// Edit the whole document as JSON in $VISUAL or $EDITOR, then save it.
    Edit,
}

pub(crate) const CONFIG_AFTER_HELP: &str = "\
Keys are config.toml's, as dotted paths; `config schema` lists them with their help.
A value is read as JSON when it parses (30, true, null, [\"a\"]); a text key also takes plain text.
Saved changes apply when openside restarts.

Examples:
  openside config get
  openside config get tickets.confirm_ttl_secs
  openside config set tickets.confirm_ttl_secs 900
  openside config set policy.mode read-only
  openside config edit";

pub(crate) async fn execute<W: Write>(
    command: &ConfigCommand,
    json: bool,
    client: &Client,
    stdout: &mut W,
) -> Result<(), String> {
    match command {
        ConfigCommand::Schema => {
            let document: Value = client.get(CONFIG).await?;
            print_value(json, stdout, &document["schema"])
        }
        ConfigCommand::Get { key } => {
            let document: Value = client.get(CONFIG).await?;
            let config = &document["config"];
            let value = match key {
                Some(key) => lookup(config, key).ok_or_else(|| unknown_key(key))?,
                None => config,
            };
            print_value(json, stdout, value)
        }
        ConfigCommand::Set { key, value } => {
            let document: Value = client.get(CONFIG).await?;
            let mut config = document["config"].clone();
            let value = parse_value(lookup(&config, key), value);
            assign(&mut config, key, value.clone())?;
            let saved = save(client, config).await?;
            if json {
                return emit(
                    stdout,
                    &json!({"key": key, "value": value, "restart_required": saved["restart_required"]}),
                );
            }
            writeln!(stdout, "{key} = {}", display(&value)).map_err(output_error)?;
            restart_note(stdout, &saved)
        }
        ConfigCommand::Edit => {
            let document: Value = client.get(CONFIG).await?;
            let before = serde_json::to_string_pretty(&document["config"])
                .map_err(|_| "invalid configuration document")?;
            let after = edit_in_editor(&before)?;
            if after.trim() == before.trim() {
                return writeln!(stdout, "no changes").map_err(output_error);
            }
            let config: Value = serde_json::from_str(&after)
                .map_err(|error| format!("the edited document is not valid JSON: {error}"))?;
            let saved = save(client, config).await?;
            if json {
                return emit(
                    stdout,
                    &json!({"restart_required": saved["restart_required"]}),
                );
            }
            writeln!(stdout, "saved").map_err(output_error)?;
            restart_note(stdout, &saved)
        }
    }
}

async fn save(client: &Client, config: Value) -> Result<Value, String> {
    client
        .json("PUT", CONFIG, Some(&json!({ "config": config })))
        .await
        .map_err(|error| {
            if error.contains("local_access_required") {
                format!("{error}. Only the computer openside runs on can change that key: run the command there, without --host/--url.")
            } else {
                error
            }
        })
}

fn restart_note<W: Write>(stdout: &mut W, saved: &Value) -> Result<(), String> {
    if saved["restart_required"] == true {
        writeln!(stdout, "Restart openside to apply it.").map_err(output_error)?;
    }
    Ok(())
}

fn unknown_key(key: &str) -> String {
    format!("no setting {key:?}; `openside config schema` lists every key")
}

fn lookup<'a>(config: &'a Value, key: &str) -> Option<&'a Value> {
    key.split('.')
        .try_fold(config, |value, part| value.as_object()?.get(part))
}

/// JSON when it parses, so numbers, booleans, lists and null keep their type; a text key takes
/// anything that is not a JSON string as plain text, so `policy.mode read-only` needs no quotes.
fn parse_value(current: Option<&Value>, text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::String(parsed)) => Value::String(parsed),
        Ok(_) if current.is_some_and(Value::is_string) => Value::String(text.to_owned()),
        Ok(parsed) => parsed,
        Err(_) => Value::String(text.to_owned()),
    }
}

/// Sets `key`, creating the sections an unset optional key sits in.
fn assign(config: &mut Value, key: &str, value: Value) -> Result<(), String> {
    let mut parts = key.split('.').peekable();
    let mut node = config;
    while let Some(part) = parts.next() {
        if part.is_empty() {
            return Err(unknown_key(key));
        }
        let object = node.as_object_mut().ok_or_else(|| unknown_key(key))?;
        if parts.peek().is_none() {
            object.insert(part.to_owned(), value);
            return Ok(());
        }
        node = object
            .entry(part)
            .or_insert_with(|| Value::Object(Map::new()));
    }
    Err(unknown_key(key))
}

fn display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn print_value<W: Write>(json: bool, stdout: &mut W, value: &Value) -> Result<(), String> {
    if json {
        return emit(stdout, value);
    }
    let text = match value {
        Value::Object(_) | Value::Array(_) => {
            serde_json::to_string_pretty(value).map_err(|_| "invalid configuration document")?
        }
        other => display(other),
    };
    writeln!(stdout, "{text}").map_err(output_error)
}

/// Writes `text` to a private temporary file, opens it in the person's editor and returns what
/// they saved.
fn edit_in_editor(text: &str) -> Result<String, String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .ok()
        .filter(|editor| !editor.trim().is_empty())
        .ok_or("set $VISUAL or $EDITOR to edit the configuration")?;
    let directory = tempfile::tempdir().map_err(|_| "could not create a temporary file")?;
    let file = directory.path().join("openside-config.json");
    write_private(&file, text)?;
    // The editor value may carry arguments (`code --wait`), so the shell splits it.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("sh")
        .arg(&file)
        .status()
        .map_err(|_| format!("could not start the editor {editor:?}"))?;
    if !status.success() {
        return Err(format!(
            "the editor {editor:?} exited without saving ({status})"
        ));
    }
    std::fs::read_to_string(&file).map_err(|_| "could not read the edited file".into())
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .map_err(|_| "could not create a temporary file")?;
    file.write_all(text.as_bytes())
        .map_err(|_| "could not write a temporary file".into())
}
