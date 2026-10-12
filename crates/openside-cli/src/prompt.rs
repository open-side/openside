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
    write!(stderr, "{label}: ")
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
