//! Terminal prompts for values that must stay out of argv and shell history: a
//! `secret_input` ticket answer and `openside vault` passphrases, usernames and passwords.
//! Hidden prompts turn terminal echo off; piped stdin is read one line per prompt.

use std::io::{IsTerminal, Read, Write};

/// Terminal echo stays off on the process's stdin until this is dropped.
struct EchoOff {
    #[cfg(unix)]
    saved: Option<nix::sys::termios::Termios>,
}

impl EchoOff {
    /// A stdin that is not a terminal (a pipe, a test reader) shows nothing, so there is
    /// nothing to turn off. A terminal whose echo cannot be turned off is refused rather than
    /// echoing the secret.
    fn new() -> Result<Self, String> {
        let stdin = std::io::stdin();
        if !stdin.is_terminal() {
            return Ok(Self {
                #[cfg(unix)]
                saved: None,
            });
        }
        #[cfg(unix)]
        {
            use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
            let saved = tcgetattr(&stdin).map_err(|_| unavailable())?;
            let mut quiet = saved.clone();
            quiet.local_flags.remove(LocalFlags::ECHO);
            tcsetattr(&stdin, SetArg::TCSANOW, &quiet).map_err(|_| unavailable())?;
            Ok(Self { saved: Some(saved) })
        }
        #[cfg(not(unix))]
        Err(unavailable())
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = &self.saved {
            let _ = nix::sys::termios::tcsetattr(
                std::io::stdin(),
                nix::sys::termios::SetArg::TCSANOW,
                saved,
            );
        }
    }
}

fn unavailable() -> String {
    "cannot hide terminal input here; pipe the value to stdin instead".to_owned()
}

/// Writes `label` to `stderr`, reads one line from `stdin` with echo off and returns it
/// without the line ending. Reads byte by byte so nothing past the line is consumed.
pub(crate) fn read_hidden<R: Read, E: Write>(
    label: &str,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<String, String> {
    read_answer(label, stdin, stderr, true)
}

/// Like [`read_hidden`] with echo left on, for values a person wants to see while typing.
pub(crate) fn read_visible<R: Read, E: Write>(
    label: &str,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<String, String> {
    read_answer(label, stdin, stderr, false)
}

fn read_answer<R: Read, E: Write>(
    label: &str,
    stdin: &mut R,
    stderr: &mut E,
    hidden: bool,
) -> Result<String, String> {
    write!(stderr, "{}: ", crate::ui::clean(label))
        .and_then(|()| stderr.flush())
        .map_err(|_| "could not write the prompt".to_owned())?;
    let echo = if hidden { Some(EchoOff::new()?) } else { None };
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        match stdin.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => line.push(byte[0]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err("could not read the answer".to_owned()),
        }
    }
    if echo.is_some() {
        drop(echo);
        // The typed Enter was not echoed either; end the prompt line.
        writeln!(stderr).map_err(|_| "could not write the prompt".to_owned())?;
    }
    let mut text = String::from_utf8(line).map_err(|_| "the answer is not UTF-8".to_owned())?;
    if text.ends_with('\r') {
        text.pop();
    }
    Ok(text)
}

pub(crate) fn form_answer<R: Read, E: Write>(
    form: &openside_proto::TicketForm,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<serde_json::Value, String> {
    use openside_proto::{FieldInitial, InputKind};
    use serde_json::{json, Map, Value};

    writeln!(
        stderr,
        "Fill fields for {} ({})",
        crate::ui::clean(&form.site_name),
        crate::ui::clean(&form.origin)
    )
    .map_err(crate::output_error)?;
    writeln!(
        stderr,
        "This fills fields only; it does not submit the page."
    )
    .map_err(crate::output_error)?;
    let mut values = Map::new();
    for field in &form.fields {
        let hidden = form.payment || crate::ui::cards::sensitive(field);
        let label = crate::ui::clean(&field.label);
        let mut prompt = label.clone();
        if let Some(origin) = &field.target_origin {
            if origin != &form.origin {
                prompt.push_str(&format!(" ({})", crate::ui::clean(origin)));
            }
        }
        if field.kind == InputKind::Checkbox {
            prompt.push_str(" [y/N]");
        }
        if !hidden {
            if let Some(initial) = &field.initial {
                let initial = match initial {
                    FieldInitial::Text(text) => crate::ui::clean(text),
                    FieldInitial::Checked(checked) => if *checked { "yes" } else { "no" }.into(),
                };
                prompt.push_str(&format!(" [blank keeps {initial}]"));
            }
        }
        let answer = if hidden {
            read_hidden(&prompt, stdin, stderr)?
        } else {
            read_visible(&prompt, stdin, stderr)?
        };
        let initial = field
            .initial
            .as_ref()
            .filter(|_| !hidden && answer.is_empty());
        let value = if let Some(initial) = initial {
            match initial {
                FieldInitial::Text(text) => Value::String(text.clone()),
                FieldInitial::Checked(checked) => Value::Bool(*checked),
            }
        } else if field.kind == InputKind::Checkbox {
            let checked = match answer.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" | "true" | "1" => true,
                "" | "n" | "no" | "false" | "0" => false,
                _ => return Err(format!("{label}: answer yes or no")),
            };
            if field.required && !checked {
                return Err(format!("{label} is required"));
            }
            Value::Bool(checked)
        } else {
            if field.required && answer.is_empty() {
                return Err(format!("{label} is required"));
            }
            if answer.chars().count() > usize::from(field.max_length) {
                return Err(format!("{label} exceeds its maximum length"));
            }
            Value::String(answer)
        };
        values.insert(field.id.clone(), value);
    }
    Ok(json!({ "values": values }))
}

pub(crate) fn information_answer<R: Read, E: Write>(
    ticket: &openside_proto::Ticket,
    stdin: &mut R,
    stderr: &mut E,
) -> Result<serde_json::Value, String> {
    if ticket.options.is_empty() {
        return read_visible("Answer", stdin, stderr).map(serde_json::Value::String);
    }
    for (index, option) in ticket.options.iter().enumerate() {
        writeln!(stderr, "{}. {}", index + 1, crate::ui::clean(&option.label))
            .map_err(crate::output_error)?;
    }
    let answer = read_visible("Option number", stdin, stderr)?;
    let picked = answer
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|n| ticket.options.get(n))
        .or_else(|| {
            ticket
                .options
                .iter()
                .find(|option| option.value == answer.trim())
        })
        .ok_or_else(|| format!("pick a number from 1 to {}", ticket.options.len()))?;
    Ok(serde_json::Value::String(picked.value.clone()))
}
