use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Remote access from other devices: pairing links and push notifications (`docs/REMOTE.md`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteConfig {
    /// HTTPS base URL other devices use to reach the daemon (Tailscale Serve or a
    /// TLS reverse proxy), e.g. `https://host.example.ts.net`. Pairing links and QR
    /// codes point here; unset prints only the pairing code and path.
    pub public_url: Option<String>,
    /// Contact URI sent to push services in the VAPID `sub` claim (`mailto:` or
    /// `https:`); defaults to `public_url` when it is HTTPS.
    pub push_contact: Option<String>,
}

/// Forwarded SMS one-time codes (`POST /v1/otp`, `otp_ingest` scope).
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct OtpConfig {
    /// Senders whose messages are accepted, compared exactly (e.g. `"+15550100"`,
    /// `"BANK"`). Empty rejects every message.
    pub senders: Vec<String>,
    /// Exact site origin (`https://login.example.com`) → regex extracting the code
    /// (first capture group, else the whole match). Only pending `secret_input`
    /// tickets whose target origin equals the key consume codes.
    pub sites: BTreeMap<String, String>,
    /// Seconds an unconsumed code is kept in memory; at least one.
    #[serde(deserialize_with = "crate::positive_seconds")]
    #[schemars(range(min = 1))]
    pub ttl_secs: u32,
}

impl Default for OtpConfig {
    fn default() -> Self {
        Self {
            senders: Vec::new(),
            sites: BTreeMap::new(),
            ttl_secs: 300,
        }
    }
}

/// Running the daemon as a login service (`openside service install`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ServiceConfig {
    /// Keep the machine awake while any session is running (`systemd-inhibit` on
    /// Linux, `caffeinate` on macOS).
    pub inhibit_sleep: bool,
    /// Override the inhibitor command (argv, no shell); it is started when the first
    /// session starts running and killed when none is. Empty uses the platform default.
    pub inhibit_cmd: Vec<String>,
}

/// Exact `http(s)://host[:port]` origin without path, query, fragment or credentials.
fn is_origin(value: &str) -> bool {
    let Some(authority) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    !authority.is_empty()
        && !authority.contains(['/', '?', '#', '@', ' '])
        && authority == authority.to_ascii_lowercase()
}

pub(crate) fn validate(config: &crate::Config) -> Result<(), (&'static str, &'static str)> {
    if let Some(url) = &config.remote.public_url {
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"));
        if rest.map_or(true, |rest| {
            rest.is_empty() || rest.contains(['?', '#', '@', ' '])
        }) {
            return Err((
                "remote.public_url",
                "must be an http(s) URL without credentials, query or fragment",
            ));
        }
    }
    if let Some(contact) = &config.remote.push_contact {
        // Push services use the VAPID `sub` claim to reach the operator: mailto: or https: only.
        let rest = contact
            .strip_prefix("mailto:")
            .or_else(|| contact.strip_prefix("https://"));
        if rest.map_or(true, |rest| {
            rest.is_empty() || rest.contains(char::is_whitespace)
        }) {
            return Err(("remote.push_contact", "must be a mailto: or https:// URL"));
        }
    }
    if config
        .otp
        .senders
        .iter()
        .any(|sender| sender.trim().is_empty())
    {
        return Err(("otp.senders", "sender ids must not be empty"));
    }
    for (origin, pattern) in &config.otp.sites {
        if !is_origin(origin) {
            return Err((
                "otp.sites",
                "keys must be exact lowercase origins such as https://login.example.com",
            ));
        }
        if regex::Regex::new(pattern).is_err() {
            return Err(("otp.sites", "pattern is not a valid regular expression"));
        }
    }
    if config
        .service
        .inhibit_cmd
        .iter()
        .any(|arg| arg.contains('\0'))
        || config
            .service
            .inhibit_cmd
            .first()
            .is_some_and(|program| program.trim().is_empty())
    {
        return Err((
            "service.inhibit_cmd",
            "provide a non-empty executable and arguments without NUL bytes",
        ));
    }
    Ok(())
}

fn loopback_host(authority: &str) -> bool {
    let host = authority
        .rsplit_once(':')
        .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
        .map_or(authority, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// `openside doctor` checks for remote access (F-4): listener exposure, TLS in front of
/// it, and the Web Push key file.
pub(crate) fn doctor_check(report: &mut crate::doctor::DoctorReport, loaded: &crate::Loaded) {
    use crate::doctor::{Check, Status};
    let mut add = |id: &str, status: Status, summary: String, next: Option<&str>| {
        report.checks.push(Check {
            id: id.into(),
            status,
            summary,
            next_step: next.map(str::to_owned),
        });
    };
    let listen = loaded.config.daemon.listen;
    match listen {
        None => add(
            "remote.listener",
            Status::Ok,
            "No TCP listener: only local Unix-socket clients can connect".into(),
            None,
        ),
        Some(address) if address.ip().is_loopback() => add(
            "remote.listener",
            Status::Ok,
            format!("TCP listener {address} is loopback-only; other devices reach it only through an HTTPS front"),
            None,
        ),
        Some(address) => add(
            "remote.listener",
            Status::Fail,
            format!("daemon.listen {address} is not a loopback address; the daemon refuses to start"),
            Some("Listen on 127.0.0.1 and publish it with Tailscale Serve or a TLS reverse proxy (docs/REMOTE.md)."),
        ),
    }
    match (&loaded.config.remote.public_url, listen) {
        (None, None) => add(
            "remote.tls",
            Status::Ok,
            "Remote access is not set up".into(),
            None,
        ),
        (None, Some(_)) => add(
            "remote.tls",
            Status::Warn,
            "remote.public_url is unset: pairing prints only the code and path, not a link or QR target".into(),
            Some("Set remote.public_url to the HTTPS address of your Tailscale Serve or reverse proxy."),
        ),
        (Some(_), None) => add(
            "remote.tls",
            Status::Fail,
            "remote.public_url is set but daemon.listen is not; the HTTPS front has nothing to forward to".into(),
            Some("Set daemon.listen = \"127.0.0.1:<port>\" and point the proxy at it."),
        ),
        (Some(url), Some(_)) => {
            if url.starts_with("https://") {
                add(
                    "remote.tls",
                    Status::Ok,
                    format!("Remote devices use {url} (TLS terminated by your proxy; tokens never cross the network in clear text)"),
                    None,
                );
            } else if url
                .strip_prefix("http://")
                .map(|rest| rest.split('/').next().unwrap_or(rest))
                .is_some_and(loopback_host)
            {
                add(
                    "remote.tls",
                    Status::Warn,
                    format!("{url} is plain HTTP on loopback: only this machine can pair"),
                    Some("Use the HTTPS address of Tailscale Serve or a TLS reverse proxy for phones."),
                );
            } else {
                add(
                    "remote.tls",
                    Status::Fail,
                    format!("{url} is plain HTTP to another host: device tokens and codes would cross the network unencrypted"),
                    Some("Put TLS in front (tailscale serve, Caddy, nginx) and set remote.public_url to its https:// address."),
                );
            }
        }
    }
    let key = loaded.paths.data_dir.join("push-vapid.key");
    match std::fs::symlink_metadata(&key) {
        Err(_) => add(
            "remote.push_keys",
            Status::Warn,
            format!(
                "No Web Push key at {}; it is created when a device first enables notifications",
                key.display()
            ),
            Some("Pair a device and enable notifications in the web app."),
        ),
        Ok(metadata) => {
            #[cfg(unix)]
            let private = {
                use std::os::unix::fs::PermissionsExt;
                metadata.is_file() && metadata.permissions().mode() & 0o077 == 0
            };
            #[cfg(not(unix))]
            let private = metadata.is_file();
            let well_formed = std::fs::read_to_string(&key)
                .ok()
                .and_then(|text| {
                    use base64::Engine;
                    base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(text.trim())
                        .ok()
                })
                .is_some_and(|bytes| p256::SecretKey::from_slice(&bytes).is_ok());
            match (private, well_formed) {
                (true, true) => add(
                    "remote.push_keys",
                    Status::Ok,
                    format!("Web Push (VAPID) key present at {} (0600)", key.display()),
                    None,
                ),
                (false, _) => add(
                    "remote.push_keys",
                    Status::Fail,
                    format!("{} is not a private regular file", key.display()),
                    Some("chmod 600 the key file, or delete it and let the daemon create a new one (devices must re-enable notifications)."),
                ),
                (true, false) => add(
                    "remote.push_keys",
                    Status::Fail,
                    format!("{} is not a valid VAPID key", key.display()),
                    Some("Delete the file; the daemon creates a new one and devices re-enable notifications."),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use crate::doctor::{DoctorReport, Status};

    fn loaded(root: &std::path::Path, edit: impl FnOnce(&mut crate::Config)) -> crate::Loaded {
        let mut config = crate::Config::default();
        edit(&mut config);
        crate::Loaded {
            config,
            paths: crate::Paths {
                config_file: root.join("config.toml"),
                data_dir: root.join("data"),
                state_dir: root.join("state"),
                runtime_dir: root.join("run"),
            },
            source: None,
            warnings: Vec::new(),
        }
    }

    fn status(loaded: &crate::Loaded, id: &str) -> Status {
        let mut report = DoctorReport::default();
        super::doctor_check(&mut report, loaded);
        report
            .checks
            .iter()
            .find(|check| check.id == id)
            .unwrap()
            .status
    }

    #[test]
    fn doctor_flags_exposed_listeners_missing_tls_and_push_key_state() {
        let root = tempfile::tempdir().unwrap();
        let base = loaded(root.path(), |_| {});
        assert_eq!(status(&base, "remote.listener"), Status::Ok);
        assert_eq!(status(&base, "remote.tls"), Status::Ok);
        let exposed = loaded(root.path(), |c| {
            c.daemon.listen = Some("0.0.0.0:8080".parse().unwrap())
        });
        assert_eq!(status(&exposed, "remote.listener"), Status::Fail);
        let loopback = loaded(root.path(), |c| {
            c.daemon.listen = Some("127.0.0.1:8080".parse().unwrap())
        });
        assert_eq!(status(&loopback, "remote.listener"), Status::Ok);
        assert_eq!(status(&loopback, "remote.tls"), Status::Warn);
        let https = loaded(root.path(), |c| {
            c.daemon.listen = Some("127.0.0.1:8080".parse().unwrap());
            c.remote.public_url = Some("https://desk.example.ts.net".into());
        });
        assert_eq!(status(&https, "remote.tls"), Status::Ok);
        let clear = loaded(root.path(), |c| {
            c.daemon.listen = Some("127.0.0.1:8080".parse().unwrap());
            c.remote.public_url = Some("http://192.168.1.5:8080".into());
        });
        assert_eq!(status(&clear, "remote.tls"), Status::Fail);
        let local = loaded(root.path(), |c| {
            c.daemon.listen = Some("127.0.0.1:8080".parse().unwrap());
            c.remote.public_url = Some("http://127.0.0.1:8080".into());
        });
        assert_eq!(status(&local, "remote.tls"), Status::Warn);
        let dangling = loaded(root.path(), |c| {
            c.remote.public_url = Some("https://desk.example.ts.net".into())
        });
        assert_eq!(status(&dangling, "remote.tls"), Status::Fail);

        assert_eq!(status(&base, "remote.push_keys"), Status::Warn);
        std::fs::create_dir_all(root.path().join("data")).unwrap();
        let key = root.path().join("data/push-vapid.key");
        // Zero is not a P-256 scalar: right length, still not a key.
        std::fs::write(&key, format!("{}\n", "A".repeat(43))).unwrap();
        assert_eq!(status(&base, "remote.push_keys"), Status::Fail);
        let real = {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7_u8; 32])
        };
        std::fs::write(&key, format!("{real}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(status(&base, "remote.push_keys"), Status::Fail);
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(status(&base, "remote.push_keys"), Status::Ok);
        std::fs::write(&key, "short").unwrap();
        assert_eq!(status(&base, "remote.push_keys"), Status::Fail);
    }

    #[test]
    fn otp_sites_and_public_url_are_validated() {
        let check = |edit: fn(&mut crate::Config)| {
            let mut config = crate::Config::default();
            edit(&mut config);
            super::validate(&config).map_err(|(key, _)| key)
        };
        assert_eq!(check(|_| {}), Ok(()));
        assert_eq!(
            check(|c| {
                c.otp
                    .sites
                    .insert("https://login.example.test/path".into(), "x".into());
            }),
            Err("otp.sites")
        );
        assert_eq!(
            check(|c| {
                c.otp
                    .sites
                    .insert("https://login.example.test".into(), "(".into());
            }),
            Err("otp.sites")
        );
        assert_eq!(
            check(|c| c.remote.public_url = Some("ftp://x".into())),
            Err("remote.public_url")
        );
        assert_eq!(
            check(|c| c.otp.senders = vec![" ".into()]),
            Err("otp.senders")
        );
        assert_eq!(
            check(|c| c.remote.push_contact = Some("ops@example.test".into())),
            Err("remote.push_contact")
        );
        assert_eq!(
            check(|c| c.remote.push_contact = Some("mailto:ops@example.test".into())),
            Ok(())
        );
    }
}
