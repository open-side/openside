//! Credential-source plugins: local manifests in `<config dir>/plugins/*.toml` that describe a
//! model relay reachable only from some networks. The daemon probes each manifest's
//! availability endpoint and, while it answers, adds the relay's accounts to the provider pools
//! as an extra source; nothing about a plugin is shown in Settings. The format is documented in
//! `docs/specs/models-accounts.md` ("Plugin sources").

use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const MAX_FILES: usize = 32;
const MAX_FILE_BYTES: u64 = 64 * 1024;
const MAX_RULES: usize = 16;
const MAX_SESSION_HEADERS: usize = 4;
/// Header names the sidecar strips and re-applies itself.
const AUTH_HEADERS: &[&str] = &[
    "x-api-key",
    "x-goog-api-key",
    "chatgpt-account-id",
    "openai-organization",
    "openai-project",
];

/// How the identity authenticates one provider's requests.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginAuth {
    /// Provider API-key header (`x-api-key`, `x-goog-api-key`).
    ApiKey,
    /// `Authorization: Bearer` with Claude Code request shaping.
    ClaudeCode,
    /// `Authorization: Bearer` on the Codex Responses route, without an upstream account id.
    CodexCli,
    /// Plain `Authorization: Bearer`.
    Bearer,
}

/// Typed outcome of a relay refusal, applied before the generic status classification.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginOutcome {
    /// The relay refuses this client shape; retrying on it cannot help.
    UnsupportedClient,
    /// An account behind the relay needs its owner to sign in again.
    SignIn,
    /// The relay has no free capacity now.
    Capacity,
    /// The relay rate-limited this caller.
    RateLimit,
}

/// One provider route of a relay.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginProvider {
    /// Appended to the manifest origin, e.g. `/v1beta`; empty for the origin itself.
    #[serde(default)]
    pub path: String,
    /// Credential shaping for this provider.
    pub auth: PluginAuth,
}

/// A header the relay wants on every request, built per session.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginSessionHeader {
    /// Lower-case `x-` header name.
    pub name: String,
    /// Value template; `{session}` and `{subagent}` expand to the request's ids.
    pub template: String,
    /// Byte limit of the expanded value, 1–256.
    #[serde(default = "default_header_length")]
    pub max_length: usize,
}

/// The availability check that gates the source.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginProbe {
    /// Path of a GET that answers 2xx only where the relay is usable.
    pub path: String,
    /// Header carrying the identity on the probe, when the endpoint wants it.
    #[serde(default)]
    pub identity_header: Option<String>,
    /// Probe deadline, 100–10000 ms (default 1500).
    #[serde(default = "default_probe_timeout")]
    pub timeout_ms: u64,
    /// Seconds between probes while available, 10–3600 (default 60); failures retry after 10 s.
    #[serde(default = "default_probe_interval")]
    pub interval_seconds: u64,
}

/// Maps one relay refusal to a typed outcome.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginRule {
    /// HTTP status, 400–599.
    pub status: u16,
    /// Matches the value of `errors.hint_header` (case-insensitive).
    #[serde(default)]
    pub hint: Option<String>,
    /// Matches the error message text (case-insensitive substring).
    #[serde(default)]
    pub body_contains: Option<String>,
    /// Outcome applied when the status and either matcher (or neither, if both are absent) match.
    pub outcome: PluginOutcome,
    /// Person-facing explanation replacing the status-only error text.
    #[serde(default)]
    pub message: Option<String>,
}

/// Relay refusal mapping.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginErrors {
    /// Response header carrying the relay's machine-readable refusal hint.
    #[serde(default)]
    pub hint_header: Option<String>,
    /// Rules in priority order; the first match wins.
    #[serde(default)]
    pub rules: Vec<PluginRule>,
}

/// One validated manifest. `identity` is the person's own value the relay identifies them
/// by; it is sent as the credential and never printed by this type.
#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PluginSource {
    /// `[a-z0-9][a-z0-9-]{1,31}`, unique across manifests; the account id in provider pools.
    pub id: String,
    /// Relay origin: http(s) scheme, host and optional port only.
    pub url: String,
    /// The person's value the relay identifies them by; sent as the provider credential.
    pub identity: String,
    /// Keyed by `anthropic`, `openai-codex` or `google`.
    pub providers: BTreeMap<String, PluginProvider>,
    /// Per-session headers, at most 4.
    #[serde(default)]
    pub session_headers: Vec<PluginSessionHeader>,
    /// Availability probe.
    pub probe: PluginProbe,
    /// Refusal mapping.
    #[serde(default)]
    pub errors: PluginErrors,
}

impl std::fmt::Debug for PluginSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginSource")
            .field("id", &self.id)
            .field("identity", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl PluginSource {
    /// `url` + the provider's path, or `None` when the manifest does not serve `provider`.
    pub fn base_url(&self, provider: &str) -> Option<String> {
        self.providers
            .get(provider)
            .map(|route| format!("{}{}", self.url, route.path))
    }

    /// Absolute probe URL.
    pub fn probe_url(&self) -> String {
        format!("{}{}", self.url, self.probe.path)
    }
}

fn default_header_length() -> usize {
    64
}
fn default_probe_timeout() -> u64 {
    1500
}
fn default_probe_interval() -> u64 {
    60
}

/// The plugin directory beside the config file.
pub fn plugin_dir(config_file: &Path) -> PathBuf {
    config_file
        .parent()
        .map(|dir| dir.join("plugins"))
        .unwrap_or_else(|| PathBuf::from("plugins"))
}

fn printable(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| (0x20..0x7f).contains(&b))
}

fn header_name(name: &str) -> bool {
    name.len() <= 64
        && name.starts_with("x-")
        && name.len() > 2
        && name[2..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !AUTH_HEADERS.contains(&name)
}

fn path(text: &str) -> bool {
    text.is_empty()
        || (text.starts_with('/')
            && !text.ends_with('/')
            && text.len() <= 128
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-._~".contains(&b)))
}

/// An http(s) origin without credentials, path, query or fragment, without a trailing slash.
pub fn normalized_origin(text: &str) -> Option<String> {
    if text.len() > 256 {
        return None;
    }
    let url = url::Url::parse(text).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().map_or(true, str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

fn validate(mut source: PluginSource) -> Option<PluginSource> {
    let id = source.id.as_bytes();
    if !(2..=32).contains(&id.len())
        || !(id[0].is_ascii_lowercase() || id[0].is_ascii_digit())
        || !id
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    {
        return None;
    }
    source.url = normalized_origin(&source.url)?;
    if !printable(&source.identity, 256) || source.identity.contains(' ') {
        return None;
    }
    if source.providers.is_empty()
        || !source.providers.iter().all(|(provider, route)| {
            path(&route.path)
                && match provider.as_str() {
                    "anthropic" => matches!(
                        route.auth,
                        PluginAuth::ClaudeCode | PluginAuth::ApiKey | PluginAuth::Bearer
                    ),
                    "openai-codex" => route.auth == PluginAuth::CodexCli,
                    "google" => matches!(route.auth, PluginAuth::ApiKey | PluginAuth::Bearer),
                    _ => false,
                }
        })
    {
        return None;
    }
    if source.session_headers.len() > MAX_SESSION_HEADERS
        || !source.session_headers.iter().all(|header| {
            header_name(&header.name)
                && printable(&header.template, 256)
                && (1..=256).contains(&header.max_length)
        })
    {
        return None;
    }
    let probe = &source.probe;
    if !path(&probe.path)
        || probe.path.is_empty()
        || !(100..=10_000).contains(&probe.timeout_ms)
        || !(10..=3600).contains(&probe.interval_seconds)
        || probe
            .identity_header
            .as_deref()
            .is_some_and(|name| !(name == "x-api-key" || header_name(name)))
    {
        return None;
    }
    let errors = &source.errors;
    if errors.rules.len() > MAX_RULES
        || errors
            .hint_header
            .as_deref()
            .is_some_and(|name| !header_name(name))
        || !errors.rules.iter().all(|rule| {
            (400..=599).contains(&rule.status)
                && rule
                    .hint
                    .as_deref()
                    .map_or(true, |hint| printable(hint, 64))
                && rule
                    .body_contains
                    .as_deref()
                    .map_or(true, |text| printable(text, 128))
                && rule
                    .message
                    .as_deref()
                    .map_or(true, |text| printable(text, 200))
                && (rule.hint.is_none() || errors.hint_header.is_some())
        })
    {
        return None;
    }
    Some(source)
}

/// Parses one manifest; `None` when it is not a valid plugin source.
pub fn parse_plugin_source(text: &str) -> Option<PluginSource> {
    validate(toml::from_str(text).ok()?)
}

/// Result of reading the plugin directory.
#[derive(Debug, Default)]
pub struct PluginLoad {
    /// Valid, trusted manifests in file-name order; a duplicate id keeps the first.
    pub sources: Vec<PluginSource>,
    /// `*.toml` files rejected as untrusted, unreadable, invalid, duplicate or over the limit.
    pub rejected: usize,
}

/// The directory must be the current user's and writable by nobody else, since a manifest is
/// fully trusted: it chooses where the identity and model traffic go.
#[cfg(unix)]
fn trusted_dir(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir).is_ok_and(|meta| {
        meta.is_dir() && meta.uid() == nix::unistd::geteuid().as_raw() && meta.mode() & 0o022 == 0
    })
}

/// Opens without following a symlink and checks the opened descriptor, so the file that is
/// checked is the file that is read. Group and other users get no access: it holds the identity.
#[cfg(unix)]
fn read_trusted(file: &Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(file)
        .ok()?;
    let meta = handle.metadata().ok()?;
    if !meta.is_file()
        || meta.uid() != nix::unistd::geteuid().as_raw()
        || meta.mode() & 0o077 != 0
        || meta.len() > MAX_FILE_BYTES
    {
        return None;
    }
    let mut text = String::new();
    handle
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= MAX_FILE_BYTES).then_some(text)
}

/// Every manifest is untrusted where ownership and modes cannot be checked.
#[cfg(not(unix))]
fn trusted_dir(_: &Path) -> bool {
    false
}
#[cfg(not(unix))]
fn read_trusted(_: &Path) -> Option<String> {
    None
}

/// Reads `dir`, rejecting every manifest when the directory is not trusted.
pub fn load_plugin_sources(dir: &Path) -> PluginLoad {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return PluginLoad::default();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    let mut load = PluginLoad::default();
    if !trusted_dir(dir) {
        load.rejected = files.len();
        return load;
    }
    load.rejected = files.len().saturating_sub(MAX_FILES);
    for file in files.into_iter().take(MAX_FILES) {
        let parsed = read_trusted(&file)
            .and_then(|text| parse_plugin_source(&text))
            .filter(|source| !load.sources.iter().any(|known| known.id == source.id));
        match parsed {
            Some(source) => load.sources.push(source),
            None => load.rejected += 1,
        }
    }
    load
}

/// `plugin.sources`: how many manifests loaded and how many origins accept a connection now.
/// Only manifest ids are named; URLs and identities stay out of the report.
pub(crate) async fn doctor_check(report: &mut crate::doctor::DoctorReport, loaded: &crate::Loaded) {
    use crate::doctor::Status;
    let PluginLoad {
        sources,
        rejected: invalid,
    } = load_plugin_sources(&plugin_dir(&loaded.paths.config_file));
    if sources.is_empty() && invalid == 0 {
        return;
    }
    let mut reachable = Vec::new();
    for source in &sources {
        let Ok(url) = url::Url::parse(&source.url) else {
            continue;
        };
        let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
            continue;
        };
        let timeout = std::time::Duration::from_millis(source.probe.timeout_ms);
        let host = host.trim_start_matches('[').trim_end_matches(']');
        if tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port)))
            .await
            .is_ok_and(|result| result.is_ok())
        {
            reachable.push(source.id.as_str());
        }
    }
    let ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
    report.add(
        "models.plugin_sources",
        if invalid > 0 { Status::Warn } else { Status::Ok },
        format!(
            "plugin sources: {} loaded, {} reachable{}{}",
            sources.len(),
            reachable.len(),
            if ids.is_empty() {
                String::new()
            } else {
                format!(" ({})", ids.join(", "))
            },
            if invalid > 0 {
                format!("; {invalid} manifest(s) rejected")
            } else {
                String::new()
            }
        ),
        (invalid > 0).then(|| {
            "Plugin manifests must be valid regular files (not symlinks) owned by you with mode 600 or 400, in a directory only you can write; see docs/specs/models-accounts.md.".into()
        }),
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const MANIFEST: &str = r#"
id = "example-relay"
url = "http://127.0.0.1:7844/"
identity = "alice"

[providers.anthropic]
auth = "claude_code"

[providers.openai-codex]
path = "/backend"
auth = "codex_cli"

[providers.google]
path = "/v1beta"
auth = "api_key"

[[session_headers]]
name = "x-example-task"
template = "openside:{session}"

[probe]
path = "/health"
identity_header = "x-api-key"

[errors]
hint_header = "x-example-hint"
[[errors.rules]]
status = 429
hint = "unsupported-client"
outcome = "unsupported_client"
"#;

    #[test]
    fn manifest_parses_with_defaults_and_redacted_debug() {
        let source = parse_plugin_source(MANIFEST).unwrap();
        assert_eq!(source.url, "http://127.0.0.1:7844");
        assert_eq!(
            source.base_url("openai-codex").as_deref(),
            Some("http://127.0.0.1:7844/backend")
        );
        assert_eq!(
            source.base_url("anthropic").as_deref(),
            Some("http://127.0.0.1:7844")
        );
        assert!(source.base_url("openai").is_none());
        assert_eq!(source.probe.timeout_ms, 1500);
        assert_eq!(source.session_headers[0].max_length, 64);
        assert!(!format!("{source:?}").contains("alice"));
    }

    #[test]
    fn manifest_rejects_unsafe_values() {
        for (from, to) in [
            ("id = \"example-relay\"", "id = \"Example\""),
            ("http://127.0.0.1:7844/", "http://user:pw@127.0.0.1:7844"),
            ("http://127.0.0.1:7844/", "http://127.0.0.1:7844/v1"),
            ("http://127.0.0.1:7844/", "ftp://127.0.0.1"),
            ("identity = \"alice\"", "identity = \"\""),
            ("auth = \"codex_cli\"", "auth = \"api_key\""),
            ("[providers.google]", "[providers.openai]"),
            ("x-example-task", "authorization"),
            ("x-example-task", "x-api-key"),
            ("path = \"/health\"", "path = \"health\""),
            ("status = 429", "status = 200"),
            ("hint_header = \"x-example-hint\"\n", ""),
            ("outcome = \"unsupported_client\"", "outcome = \"retry\""),
            ("[probe]", "[probe]\nunknown = 1"),
        ] {
            let changed = MANIFEST.replacen(from, to, 1);
            assert!(parse_plugin_source(&changed).is_none(), "{to}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn loader_keeps_first_duplicate_and_counts_rejections() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.toml", MANIFEST, 0o600);
        write(dir.path(), "b.toml", MANIFEST, 0o600);
        write(dir.path(), "c.toml", "id = 1", 0o600);
        write(dir.path(), "notes.txt", "ignored", 0o600);
        let load = load_plugin_sources(dir.path());
        assert_eq!(load.sources.len(), 1);
        assert_eq!(load.rejected, 2);
        assert_eq!(
            load_plugin_sources(&dir.path().join("missing"))
                .sources
                .len(),
            0
        );
    }

    #[cfg(unix)]
    fn write(dir: &Path, name: &str, text: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn loader_rejects_untrusted_files_and_directories() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "target.toml", MANIFEST, 0o600);
        std::os::unix::fs::symlink(
            outside.path().join("target.toml"),
            dir.path().join("link.toml"),
        )
        .unwrap();
        write(dir.path(), "group-write.toml", MANIFEST, 0o620);
        write(dir.path(), "world-write.toml", MANIFEST, 0o602);
        write(dir.path(), "group-read.toml", MANIFEST, 0o640);
        write(dir.path(), "world-read.toml", MANIFEST, 0o604);
        let load = load_plugin_sources(dir.path());
        assert_eq!((load.sources.len(), load.rejected), (0, 5));

        write(dir.path(), "private.toml", MANIFEST, 0o400);
        let load = load_plugin_sources(dir.path());
        assert_eq!((load.sources.len(), load.rejected), (1, 5));

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o770)).unwrap();
        let load = load_plugin_sources(dir.path());
        assert_eq!((load.sources.len(), load.rejected), (0, 6));
    }
}
