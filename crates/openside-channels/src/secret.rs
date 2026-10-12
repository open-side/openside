use std::{fmt, time::Duration};

use tokio::process::Command;

use crate::error::{ChannelError, Result};

const COMMAND_DEADLINE: Duration = Duration::from_secs(10);

/// A credential read from its source. Never stored by openside, never logged.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    /// Wrap a value; callers keep it out of logs.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw value, for building an `Authorization` header or a signature.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Read a secret from `*_cmd` argv (stdout, no shell) or the named environment
/// variable. The command wins, matching `vault.passphrase_cmd` style sources.
/// Errors name only `label` and the source kind.
pub async fn resolve(
    label: &str,
    env_name: Option<&str>,
    command: Option<&[String]>,
    env: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<Secret> {
    let value = if let Some(argv) = command {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| ChannelError::Config(format!("{label}: empty command")))?;
        let output = tokio::time::timeout(
            COMMAND_DEADLINE,
            Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| ChannelError::Config(format!("{label}: secret command timed out")))?
        .map_err(|_| ChannelError::Config(format!("{label}: secret command could not start")))?;
        if !output.status.success() {
            return Err(ChannelError::Config(format!(
                "{label}: secret command failed"
            )));
        }
        String::from_utf8(output.stdout).map_err(|_| {
            ChannelError::Config(format!("{label}: secret command output is not UTF-8"))
        })?
    } else if let Some(name) = env_name {
        env(name).ok_or_else(|| {
            ChannelError::Config(format!("{label}: environment variable {name} is not set"))
        })?
    } else {
        return Err(ChannelError::Config(format!(
            "{label}: configure a *_env variable name or a *_cmd argv"
        )));
    };
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err(ChannelError::Config(format!("{label}: secret is empty")));
    }
    Ok(Secret(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[tokio::test]
    async fn command_output_is_trimmed_and_debug_is_redacted() {
        let argv = vec!["echo".to_owned(), "tok-123".to_owned()];
        let secret = resolve("test token", None, Some(&argv), &no_env).await;
        let secret = secret.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(secret.expose(), "tok-123");
        assert!(!format!("{secret:?}").contains("tok-123"));
    }

    #[tokio::test]
    async fn env_name_is_read_and_missing_source_names_no_value() {
        let env = |name: &str| (name == "BOT").then(|| " value \n".to_owned());
        let secret = resolve("t", Some("BOT"), None, &env).await;
        assert_eq!(secret.unwrap_or_else(|e| panic!("{e}")).expose(), "value");
        let missing = resolve("t", Some("NOPE"), None, &no_env).await;
        assert!(missing.is_err_and(|e| e.to_string().contains("NOPE")));
        assert!(resolve("t", None, None, &no_env).await.is_err());
    }

    #[tokio::test]
    async fn failing_or_empty_command_is_an_error_without_output() {
        let failing = vec!["false".to_owned()];
        assert!(resolve("t", None, Some(&failing), &no_env).await.is_err());
        let empty = vec!["true".to_owned()];
        assert!(resolve("t", None, Some(&empty), &no_env).await.is_err());
    }
}
