use std::{fmt, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// End-to-end encrypted sync of settings, pinned tabs, vault items and selected memories
/// (`docs/specs/sync.md`). Exactly one backend may be set; neither disables sync.
#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SyncConfig {
    /// WebDAV collection URL (any WebDAV share); https, or http on loopback only.
    pub webdav_url: Option<String>,
    /// WebDAV HTTP Basic username.
    pub webdav_username: Option<String>,
    /// WebDAV password command argv, executed without a shell; never logged.
    pub webdav_password_cmd: Option<Vec<String>>,
    /// Shared directory synced by other means (Syncthing, network share) instead of WebDAV.
    pub dir: Option<PathBuf>,
    /// Sync passphrase command argv, executed without a shell. The derived key stays in
    /// daemon memory; output must never be logged. Doctor never executes this command.
    pub passphrase_cmd: Option<Vec<String>>,
    /// Sync the live policy document (`/v1/policy`). Off by default: a paired device could
    /// otherwise widen this device's allowed origins or default mode.
    pub settings: bool,
    /// Memories carrying any of these tags are synced; empty syncs no memories.
    pub memory_tags: Vec<String>,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            webdav_url: None,
            webdav_username: None,
            webdav_password_cmd: None,
            dir: None,
            passphrase_cmd: None,
            settings: false,
            memory_tags: vec!["sync".to_owned()],
        }
    }
}

impl fmt::Debug for SyncConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncConfig")
            .field("webdav_url", &self.webdav_url)
            .field("webdav_username", &self.webdav_username)
            .field(
                "webdav_password_cmd",
                &self.webdav_password_cmd.as_ref().map(|_| "[redacted]"),
            )
            .field("dir", &self.dir)
            .field(
                "passphrase_cmd",
                &self.passphrase_cmd.as_ref().map(|_| "[redacted]"),
            )
            .field("settings", &self.settings)
            .field("memory_tags", &self.memory_tags)
            .finish()
    }
}

fn argv_valid(argv: &[String]) -> bool {
    !argv.is_empty() && !argv[0].trim().is_empty() && !argv.iter().any(|a| a.contains('\0'))
}

impl SyncConfig {
    /// Whether a backend is configured.
    pub fn enabled(&self) -> bool {
        self.webdav_url.is_some() || self.dir.is_some()
    }

    pub(crate) fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        if self.webdav_url.is_some() && self.dir.is_some() {
            return Err((
                "sync.dir",
                "set either sync.webdav_url or sync.dir, not both",
            ));
        }
        if self.dir.as_ref().is_some_and(|d| d.as_os_str().is_empty()) {
            return Err(("sync.dir", "path must not be empty"));
        }
        if let Some(url) = &self.webdav_url {
            if !webdav_url_allowed(url) {
                return Err((
                    "sync.webdav_url",
                    "use https, or http on a loopback address, without credentials or query",
                ));
            }
        }
        if self.webdav_username.is_some() != self.webdav_password_cmd.is_some() {
            return Err((
                "sync.webdav_password_cmd",
                "set webdav_username and webdav_password_cmd together",
            ));
        }
        if self
            .webdav_username
            .as_ref()
            .is_some_and(|u| u.is_empty() || u.contains(':') || u.chars().any(char::is_control))
        {
            return Err((
                "sync.webdav_username",
                "must be non-empty without ':' or control characters",
            ));
        }
        if self
            .webdav_password_cmd
            .as_ref()
            .is_some_and(|a| !argv_valid(a))
        {
            return Err((
                "sync.webdav_password_cmd",
                "provide a non-empty executable and arguments without NUL bytes",
            ));
        }
        if self.passphrase_cmd.as_ref().is_some_and(|a| !argv_valid(a)) {
            return Err((
                "sync.passphrase_cmd",
                "provide a non-empty executable and arguments without NUL bytes",
            ));
        }
        if self
            .memory_tags
            .iter()
            .any(|t| t.trim().is_empty() || t.trim() != t || t.chars().count() > 64)
        {
            return Err((
                "sync.memory_tags",
                "tags must be 1-64 characters without outer spaces",
            ));
        }
        Ok(())
    }
}

/// The share only ever sees ciphertext, but plain http beyond loopback would expose the
/// WebDAV password to the network.
fn webdav_url_allowed(url: &str) -> bool {
    let (rest, https) = if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') || url.contains('?') || url.contains('#') {
        return false;
    }
    if https {
        return true;
    }
    let host = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .map(|h| format!("{h}]"))
            .unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default().to_owned()
    };
    host == "localhost"
        || host == "[::1]"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn webdav(url: &str) -> SyncConfig {
        SyncConfig {
            webdav_url: Some(url.into()),
            ..SyncConfig::default()
        }
    }

    #[test]
    fn webdav_url_requires_https_off_loopback_and_one_backend() {
        for ok in [
            "https://sync.example.test",
            "https://sync.example.test:8443/base",
            "http://127.0.0.1:9000",
            "http://localhost:9000/",
            "http://[::1]:9000",
        ] {
            assert_eq!(webdav(ok).validate(), Ok(()), "{ok}");
        }
        for bad in [
            "http://sync.example.test",
            "http://127.example.test",
            "https://user:pw@sync.example.test",
            "https://sync.example.test/?token=x",
            "ftp://sync.example.test",
            "sync.example.test",
        ] {
            assert_eq!(
                webdav(bad).validate().map_err(|e| e.0),
                Err("sync.webdav_url"),
                "{bad}"
            );
        }
        let both = SyncConfig {
            dir: Some("shared".into()),
            ..webdav("https://sync.example.test")
        };
        assert_eq!(both.validate().map_err(|e| e.0), Err("sync.dir"));
        let empty_cmd = SyncConfig {
            passphrase_cmd: Some(vec![String::new()]),
            ..SyncConfig::default()
        };
        assert_eq!(
            empty_cmd.validate().map_err(|e| e.0),
            Err("sync.passphrase_cmd")
        );
        let user_only = SyncConfig {
            webdav_username: Some("alice".into()),
            ..webdav("https://dav.example.test")
        };
        assert_eq!(
            user_only.validate().map_err(|e| e.0),
            Err("sync.webdav_password_cmd")
        );
        let defaults = SyncConfig::default();
        assert!(!defaults.settings && defaults.memory_tags == ["sync"] && !defaults.enabled());
    }
}
