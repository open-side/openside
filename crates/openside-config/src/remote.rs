use std::{collections::BTreeMap, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// Remote access from other devices: transports, pairing links and push notifications
/// (`docs/REMOTE.md`).
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteConfig {
    /// Managed transports, in pairing preference order: `"tailscale"`, `"cloudflare"`,
    /// `"relay"`; a single string, a list, or `"off"` (none). Several may run at once.
    #[serde(deserialize_with = "modes")]
    pub mode: Vec<RemoteMode>,
    /// HTTPS base URL other devices use to reach the daemon directly (Tailscale Serve or a
    /// TLS reverse proxy), e.g. `https://host.example.ts.net`. Pairing links and QR
    /// codes for the `tailscale` transport point here; unset prints only the pairing code
    /// and path.
    pub public_url: Option<String>,
    /// Contact URI sent to push services in the VAPID `sub` claim (`mailto:` or
    /// `https:`); defaults to `public_url` when it is HTTPS.
    pub push_contact: Option<String>,
    /// Cloudflare Tunnel run by the daemon (`mode` contains `"cloudflare"`).
    pub cloudflare: CloudflareConfig,
    /// Self-hosted relay the daemon registers with (`mode` contains `"relay"`).
    pub relay: RelayConfig,
}

/// A way for devices outside this network to reach the daemon.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RemoteMode {
    /// Tailscale Serve in front of `daemon.listen`; devices use `remote.public_url`.
    Tailscale,
    /// `cloudflared` supervised by the daemon, forwarding to `daemon.listen`.
    Cloudflare,
    /// Outbound WebSocket to a relay; end-to-end encrypted to paired devices.
    Relay,
}

fn modes<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<RemoteMode>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Modes {
        One(String),
        Many(Vec<RemoteMode>),
    }
    let mut list = match Modes::deserialize(deserializer)? {
        Modes::One(text) if text == "off" => Vec::new(),
        Modes::One(text) => vec![RemoteMode::deserialize(
            serde::de::value::StrDeserializer::<D::Error>::new(&text),
        )?],
        Modes::Many(list) => list,
    };
    let mut seen = Vec::new();
    list.retain(|mode| {
        let fresh = !seen.contains(mode);
        seen.push(*mode);
        fresh
    });
    Ok(list)
}

/// How the daemon runs `cloudflared`.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CloudflareTunnel {
    /// Account-less quick tunnel: a random `https://*.trycloudflare.com` URL that changes
    /// whenever `cloudflared` restarts (pair again after a restart).
    #[default]
    Quick,
    /// Named tunnel from the Cloudflare dashboard: stable hostname, needs its token.
    Named,
}

/// `[remote.cloudflare]`.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CloudflareConfig {
    /// `quick` (zero configuration) or `named` (stable hostname).
    pub tunnel: CloudflareTunnel,
    /// Public hostname routed to this tunnel in the Cloudflare dashboard (`named` only),
    /// e.g. `desk.example.com`.
    pub hostname: Option<String>,
    /// File holding the named tunnel's token (0600). `OPENSIDE_CLOUDFLARE_TOKEN` is used
    /// when unset. The token is passed to `cloudflared` in its environment, never argv.
    pub token_file: Option<PathBuf>,
    /// `cloudflared` executable (name on `PATH` or absolute path).
    pub bin: String,
}

impl Default for CloudflareConfig {
    fn default() -> Self {
        Self {
            tunnel: CloudflareTunnel::Quick,
            hostname: None,
            token_file: None,
            bin: "cloudflared".into(),
        }
    }
}

/// `[remote.relay]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RelayConfig {
    /// Relay origin, e.g. `https://relay.example.com` (`http://` only on loopback).
    pub url: Option<String>,
    /// File holding the registration token the relay operator issued (0600).
    /// `OPENSIDE_RELAY_TOKEN` is used when unset.
    pub token_file: Option<PathBuf>,
}

impl RemoteConfig {
    /// Whether `mode` lists `which`.
    pub fn has(&self, which: RemoteMode) -> bool {
        self.mode.contains(&which)
    }
}

/// Expand a leading `~/` with `$HOME`; other paths are returned unchanged.
pub fn expand_home(path: &std::path::Path) -> PathBuf {
    match (path.strip_prefix("~"), std::env::var_os("HOME")) {
        (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => path.to_owned(),
    }
}

/// Read a secret from `file` (trimmed), else from the environment variable `env`.
pub fn read_secret(file: Option<&std::path::Path>, env: &str) -> Option<String> {
    let value = match file {
        Some(path) => std::fs::read_to_string(expand_home(path)).ok()?,
        None => std::env::var(env).ok()?,
    };
    let value = value.trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn https_or_loopback(url: &str) -> bool {
    if let Some(rest) = url.strip_prefix("https://") {
        let host = rest.trim_end_matches('/');
        return !host.is_empty() && !host.contains(['/', '?', '#', '@', ' ']);
    }
    url.strip_prefix("http://")
        .map(|rest| rest.trim_end_matches('/'))
        .is_some_and(|rest| !rest.contains(['/', '?', '#', '@', ' ']) && loopback_host(rest))
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
    let remote = &config.remote;
    if let Some(url) = &remote.relay.url {
        if !https_or_loopback(url) {
            return Err((
                "remote.relay.url",
                "must be an https:// origin without a path (http:// only on loopback)",
            ));
        }
    }
    if remote.has(RemoteMode::Relay) && remote.relay.url.is_none() {
        return Err((
            "remote.relay.url",
            "is required when remote.mode includes \"relay\"",
        ));
    }
    if let Some(hostname) = &remote.cloudflare.hostname {
        if hostname.is_empty()
            || hostname.contains(['/', ':', '?', '#', '@', ' '])
            || hostname != &hostname.to_ascii_lowercase()
        {
            return Err((
                "remote.cloudflare.hostname",
                "must be a bare lowercase host name such as desk.example.com",
            ));
        }
    }
    if remote.has(RemoteMode::Cloudflare) {
        if remote.cloudflare.tunnel == CloudflareTunnel::Named
            && remote.cloudflare.hostname.is_none()
        {
            return Err((
                "remote.cloudflare.hostname",
                "is required for a named tunnel",
            ));
        }
        if remote.cloudflare.bin.trim().is_empty() || remote.cloudflare.bin.contains('\0') {
            return Err((
                "remote.cloudflare.bin",
                "must name the cloudflared executable",
            ));
        }
    }
    let needs_listener = remote.has(RemoteMode::Cloudflare) || remote.has(RemoteMode::Tailscale);
    if needs_listener && config.daemon.listen.is_none() {
        return Err((
            "daemon.listen",
            "is required for the tailscale and cloudflare transports (they forward to it)",
        ));
    }
    if remote.has(RemoteMode::Tailscale)
        && !remote
            .public_url
            .as_deref()
            .is_some_and(|url| url.starts_with("https://"))
    {
        return Err((
            "remote.public_url",
            "must be the https:// Tailscale Serve address when remote.mode includes \"tailscale\"",
        ));
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
    transport_checks(&mut add, loaded);
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

#[cfg(unix)]
fn private_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::symlink_metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn private_file(path: &std::path::Path) -> bool {
    path.is_file()
}

/// A secret from a 0600 file or the environment: `Ok(source)` or `Err((summary, next step))`.
fn secret_check(
    file: Option<&std::path::Path>,
    env: &str,
    what: &str,
) -> Result<String, (String, String)> {
    match file {
        Some(path) => {
            let path = expand_home(path);
            if !path.is_file() {
                return Err((
                    format!("{what} file {} does not exist", path.display()),
                    format!("Write the {what} to {} and chmod 600 it.", path.display()),
                ));
            }
            if !private_file(&path) {
                return Err((
                    format!("{what} file {} is readable by other users", path.display()),
                    format!("chmod 600 {}", path.display()),
                ));
            }
            if read_secret(Some(&path), env).is_none() {
                return Err((
                    format!("{what} file {} is empty", path.display()),
                    format!("Write the {what} to it."),
                ));
            }
            Ok(path.display().to_string())
        }
        None if read_secret(None, env).is_some() => Ok(format!("${env}")),
        None => Err((
            format!("no {what}: neither a token_file nor ${env} is set"),
            format!("Set the token_file key or export {env} for the daemon."),
        )),
    }
}

fn transport_checks(
    add: &mut impl FnMut(&str, crate::doctor::Status, String, Option<&str>),
    loaded: &crate::Loaded,
) {
    use crate::doctor::Status;
    let remote = &loaded.config.remote;
    if remote.has(RemoteMode::Tailscale) {
        let url = remote.public_url.as_deref().unwrap_or_default();
        let host = url
            .strip_prefix("https://")
            .unwrap_or_default()
            .trim_end_matches('/');
        match (
            crate::discovery::find_on_path("tailscale"),
            host.ends_with(".ts.net"),
        ) {
            (None, _) => add(
                "remote.tailscale",
                Status::Fail,
                "tailscale is not installed or not on PATH".into(),
                Some("Install Tailscale, log in, then run `openside remote setup tailscale`."),
            ),
            (Some(_), true) => add(
                "remote.tailscale",
                Status::Ok,
                format!("Tailscale Serve address {url}"),
                None,
            ),
            (Some(_), false) => add(
                "remote.tailscale",
                Status::Warn,
                format!("{url} is not a *.ts.net address; is this really the Tailscale Serve URL?"),
                Some("Run `openside remote setup tailscale` to read it from `tailscale status`."),
            ),
        }
    }
    if remote.has(RemoteMode::Cloudflare) {
        let cloudflare = &remote.cloudflare;
        let token = (cloudflare.tunnel == CloudflareTunnel::Named).then(|| {
            secret_check(
                cloudflare.token_file.as_deref(),
                "OPENSIDE_CLOUDFLARE_TOKEN",
                "Cloudflare tunnel token",
            )
        });
        match (crate::discovery::find_on_path(&cloudflare.bin), token) {
            (None, _) => add(
                "remote.cloudflare",
                Status::Fail,
                format!("{} is not installed or not on PATH", cloudflare.bin),
                Some("Install cloudflared (https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/)."),
            ),
            (Some(_), Some(Err((summary, next)))) => {
                add("remote.cloudflare", Status::Fail, summary, Some(&next));
            }
            (Some(bin), Some(Ok(source))) => add(
                "remote.cloudflare",
                Status::Ok,
                format!(
                    "Named tunnel https://{} via {} (token from {source})",
                    cloudflare.hostname.as_deref().unwrap_or_default(),
                    bin.display()
                ),
                None,
            ),
            (Some(bin), None) => add(
                "remote.cloudflare",
                Status::Ok,
                format!(
                    "Quick tunnel via {}: the trycloudflare.com address changes whenever it restarts, so pair again afterwards",
                    bin.display()
                ),
                None,
            ),
        }
    }
    if remote.has(RemoteMode::Relay) {
        let url = remote.relay.url.as_deref().unwrap_or_default();
        let key = loaded.paths.data_dir.join("relay-key");
        match secret_check(remote.relay.token_file.as_deref(), "OPENSIDE_RELAY_TOKEN", "relay registration token") {
            Err((summary, next)) => add("remote.relay", Status::Fail, summary, Some(&next)),
            Ok(_) if key.exists() && !private_file(&key) => add(
                "remote.relay",
                Status::Fail,
                format!("{} is not a private regular file", key.display()),
                Some("chmod 600 it, or delete it and pair every relay device again."),
            ),
            Ok(source) if key.exists() => add(
                "remote.relay",
                Status::Ok,
                format!("Relay {url} (token from {source}); end-to-end key {}", key.display()),
                None,
            ),
            Ok(source) => add(
                "remote.relay",
                Status::Warn,
                format!("Relay {url} (token from {source}); the end-to-end key is created when the daemon starts"),
                Some("Restart the daemon, then pair devices with `openside pair --via relay`."),
            ),
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

    #[test]
    fn remote_mode_accepts_a_string_a_list_or_off_and_validates_each_transport() {
        use super::{CloudflareTunnel, RemoteMode};
        let parse = |text: &str| -> crate::RemoteConfig { toml::from_str(text).unwrap() };
        assert_eq!(parse("mode = \"relay\"").mode, vec![RemoteMode::Relay]);
        assert_eq!(parse("mode = \"off\"").mode, Vec::<RemoteMode>::new());
        assert_eq!(
            parse("mode = [\"tailscale\", \"relay\", \"tailscale\"]").mode,
            vec![RemoteMode::Tailscale, RemoteMode::Relay]
        );
        assert!(toml::from_str::<crate::RemoteConfig>("mode = \"carrier-pigeon\"").is_err());

        let check = |edit: fn(&mut crate::Config)| {
            let mut config = crate::Config::default();
            edit(&mut config);
            super::validate(&config).map_err(|(key, _)| key)
        };
        assert_eq!(
            check(|c| c.remote.mode = vec![RemoteMode::Relay]),
            Err("remote.relay.url")
        );
        assert_eq!(
            check(|c| {
                c.remote.mode = vec![RemoteMode::Relay];
                c.remote.relay.url = Some("http://relay.example.test".into());
            }),
            Err("remote.relay.url")
        );
        assert_eq!(
            check(|c| {
                c.remote.mode = vec![RemoteMode::Relay];
                c.remote.relay.url = Some("https://relay.example.test".into());
            }),
            Ok(())
        );
        assert_eq!(
            check(|c| {
                c.remote.mode = vec![RemoteMode::Relay];
                c.remote.relay.url = Some("http://127.0.0.1:9000".into());
            }),
            Ok(())
        );
        assert_eq!(
            check(|c| c.remote.mode = vec![RemoteMode::Cloudflare]),
            Err("daemon.listen")
        );
        assert_eq!(
            check(|c| {
                c.remote.mode = vec![RemoteMode::Cloudflare];
                c.daemon.listen = Some("127.0.0.1:7777".parse().unwrap());
                c.remote.cloudflare.tunnel = CloudflareTunnel::Named;
            }),
            Err("remote.cloudflare.hostname")
        );
        assert_eq!(
            check(|c| {
                c.remote.mode = vec![RemoteMode::Cloudflare];
                c.daemon.listen = Some("127.0.0.1:7777".parse().unwrap());
                c.remote.cloudflare.hostname = Some("https://desk.example.com".into());
            }),
            Err("remote.cloudflare.hostname")
        );
        assert_eq!(
            check(|c| {
                c.remote.mode = vec![RemoteMode::Tailscale];
                c.daemon.listen = Some("127.0.0.1:7777".parse().unwrap());
            }),
            Err("remote.public_url")
        );
    }

    #[test]
    fn doctor_reports_each_enabled_transport() {
        use super::RemoteMode;
        let root = tempfile::tempdir().unwrap();
        let token = root.path().join("relay-token");
        std::fs::write(&token, "registration-token-0123456789\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let relay = loaded(root.path(), |c| {
            c.remote.mode = vec![RemoteMode::Relay];
            c.remote.relay.url = Some("https://relay.example.test".into());
        });
        let mut relay = relay;
        relay.config.remote.relay.token_file = Some(token.clone());
        #[cfg(unix)]
        {
            assert_eq!(status(&relay, "remote.relay"), Status::Fail);
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(status(&relay, "remote.relay"), Status::Warn);
        std::fs::create_dir_all(root.path().join("data")).unwrap();
        let key = root.path().join("data/relay-key");
        std::fs::write(&key, "key").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(status(&relay, "remote.relay"), Status::Ok);

        let cloudflare = loaded(root.path(), |c| {
            c.remote.mode = vec![RemoteMode::Cloudflare];
            c.remote.cloudflare.bin = "/nonexistent/cloudflared".into();
        });
        assert_eq!(status(&cloudflare, "remote.cloudflare"), Status::Fail);
        let off = loaded(root.path(), |_| {});
        let mut report = DoctorReport::default();
        super::doctor_check(&mut report, &off);
        assert!(!report
            .checks
            .iter()
            .any(|c| c.id.starts_with("remote.relay") || c.id == "remote.cloudflare"));
    }
}
